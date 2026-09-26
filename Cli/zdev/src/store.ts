// Config (TOML) + session store — same semantics as zcore (contract §1, §6).
import fs from "node:fs";
import path from "node:path";
import * as catalog from "./catalog";
import * as creds from "./creds";
import * as devin from "./devin";
import * as live from "./live";
import * as proto from "./proto";
import * as toml from "./toml";

export type Config = {
  default_model: string | null;
  providers: Record<string, any>;
  models: Record<string, any>;
  permissions: { allow: string[]; ask: string[] };
  subagents: { max_parallel: number };
  bridge: { large_read_lines: number; summarizer_model: string };
  mcp: Record<string, any>;
  tools: Record<string, any>;
  hooks: any[];
  [k: string]: any;
};

export function sanitizeTomlText(s: string): string {
  return s.replace(/\0/g, "").replace(/\\x[0-9a-fA-F]{2}/g, "");
}

export function tomlQuote(value: string): string {
  const clean = [...value].filter((c) => c !== "\0" && (c >= " " || c === "\t")).join("");
  return JSON.stringify(clean);
}

export function loadConfig(home: string): Config {
  const cfg: Config = {
    default_model: null,
    providers: {},
    models: {},
    // a fresh install (no [permissions] table): read-only tools run, anything
    // that changes files or runs commands asks first — like config.example.toml
    permissions: {
      allow: ["read", "grep", "glob", "ls", "memory_search", "tool_search", "context_remaining"],
      ask: ["write", "edit", "bash", "memory_write", "task", "mcp_call"],
    },
    subagents: { max_parallel: 3 },
    bridge: { large_read_lines: 200, summarizer_model: "" },
    mcp: {},
    tools: {},
    hooks: [],
  };
  const p = path.join(home, "config.toml");
  if (fs.existsSync(p) && fs.statSync(p).isFile()) {
    try {
      const loaded = toml.parse(sanitizeTomlText(fs.readFileSync(p, "utf8")));
      for (const [k, v] of Object.entries(loaded)) {
        const cur = (cfg as any)[k];
        if (k === "permissions" && v && typeof v === "object") {
          // an explicit table replaces the fresh-install default entirely
          const pv = v as Record<string, unknown>;
          cfg.permissions = {
            allow: Array.isArray(pv.allow) ? (pv.allow as string[]) : [],
            ask: Array.isArray(pv.ask) ? (pv.ask as string[]) : [],
          };
        } else if (
          cur !== null &&
          typeof cur === "object" &&
          !Array.isArray(cur) &&
          v !== null &&
          typeof v === "object" &&
          !Array.isArray(v)
        ) {
          (cfg as any)[k] = { ...cur, ...v };
        } else {
          (cfg as any)[k] = v;
        }
      }
    } catch (e) {
      console.error(`config.toml parse error: ${e} — using defaults`);
    }
  }
  if (Object.keys(cfg.providers).length === 0) {
    cfg.providers.fixture = { kind: "fixture", base_url: "", api_key_env: "" };
  }
  if (Object.keys(cfg.models).length === 0) {
    cfg.models.fixture = {
      provider: "fixture",
      id: "fixture",
      context: 128000,
      price: { input: 0, cache_read: 0, cache_write: 0, output: 0 },
    };
    // keep a configured default (e.g. a catalog ref like anthropic/claude-opus-5-5
    // written without a [models] table)
    if (!cfg.default_model) cfg.default_model = "fixture";
  }
  return cfg;
}

/** §12 outbound allowlist: configured provider hosts, the built-in catalog's
 * hosts, and the ChatGPT backend + sign-in hosts. */
export function allowedHosts(cfg: Config): string[] {
  const out = new Set<string>();
  const add = (url: string) => {
    const h = catalog.hostOf(url);
    if (h) out.add(h);
  };
  for (const p of Object.values(cfg.providers)) {
    if (p?.base_url?.includes("://")) add(p.base_url);
    // subscription kind talks to the OAuth issuer for token refresh too
    if (p?.kind === "chatgpt") out.add("auth.openai.com");
  }
  for (const p of catalog.providers()) add(p.base_url);
  out.add("chatgpt.com");
  out.add("auth.openai.com");
  out.add("server.codeium.com");
  out.add("inference.codeium.com");
  out.add("api.devin.ai");
  return [...out];
}

/** A catalog provider is usable once it has a key (or a ChatGPT / Devin sign-in). */
export function providerReady(p: catalog.ProviderDef): boolean {
  if (p.kind === "chatgpt") return !!creds.chatgpt();
  if (p.kind === "devin") return devin.knownReady() || (live.cached("devin")?.length ?? 0) > 0;
  if (!p.env.length) return false; // local servers list their models via config.toml
  return !!creds.keyFor(p.id, p.env);
}

const priceObj = (p?: number[]) => ({
  input: p?.[0] ?? 0,
  output: p?.[1] ?? 0,
  cache_read: p?.[2] ?? 0,
  cache_write: p?.[3] ?? 0,
});

/**
 * Resolve a model alias → [provider cfg, model id]. Two forms: a [models]
 * alias from config.toml (`r9`), or a catalog reference `provider/model`
 * (`anthropic/claude-opus-5-5`, `chatgpt/gpt-5.5`, `openrouter/qwen/…`).
 * Keys come from env vars, the credential store or credentials.json.
 */
export function providerFor(cfg: Config, alias: string): [Record<string, any>, string] | null {
  const home = proto.cliHome();
  const m = cfg.models[alias];
  if (m) {
    const p = cfg.providers[m.provider];
    if (!p) return null;
    const envOk = !!(p.api_key_env && process.env[p.api_key_env]);
    return [
      {
        kind: p.kind ?? "fixture",
        base_url: p.base_url ?? "",
        api_key_env: p.api_key_env ?? "",
        api_key: envOk || p.kind === "fixture" ? null : (creds.storedKey(m.provider)?.[0] ?? null),
        model: m.id,
        context: m.context ?? 128000,
        output: m.output ?? 0,
        prices: m.price ?? {},
        effort: m.effort ?? null,
        home,
      },
      m.id,
    ];
  }
  const ref = catalog.splitRef(alias);
  if (!ref) return null;
  const [pid, mid] = ref;
  const def = catalog.provider(pid);
  const known = def ? (live.cached(pid)?.find((x) => x.id === mid) ?? catalog.model(def, mid)) : undefined;
  // a config.toml provider of that name wins over the catalog entry
  const p = cfg.providers[pid];
  if (p) {
    return [
      {
        kind: p.kind ?? "fixture",
        base_url: p.base_url ?? "",
        api_key_env: p.api_key_env ?? "",
        api_key: creds.storedKey(pid)?.[0] ?? null,
        model: mid,
        context: known?.context ?? 128000,
        output: known?.output ?? 0,
        prices: priceObj(known?.price),
        effort: null,
        home,
      },
      mid,
    ];
  }
  if (!def) return null;
  return [
    {
      kind: def.kind,
      base_url: def.base_url,
      api_key_env: def.env[0] ?? "",
      api_key: catalog.isSubscription(def) ? null : (creds.keyFor(pid, def.env)?.[0] ?? null),
      model: mid,
      context: known?.context ?? 128000,
      output: known?.output ?? 0,
      prices: priceObj(known?.price),
      effort: null,
      home,
    },
    mid,
  ];
}

export type ModelChoice = {
  /** what --model / session/set_model takes */
  alias: string;
  provider: string;
  providerName: string;
  id: string;
  name: string;
  context: number;
  price: { input: number; output: number; cache_read: number; cache_write: number };
  free: boolean;
  reasoning: boolean;
  kind: string;
  fromConfig: boolean;
};

/** Everything usable right now: config.toml aliases first, then every
 * catalog provider with a key — its live list when one was fetched. */
export function availableModels(cfg: Config): ModelChoice[] {
  const out: ModelChoice[] = Object.keys(cfg.models)
    .sort()
    .map((alias) => {
      const m = cfg.models[alias];
      const price = { ...priceObj(), ...(m.price ?? {}) };
      return {
        alias,
        provider: m.provider ?? "",
        providerName: m.provider ?? "",
        id: m.id ?? "",
        name: alias,
        context: m.context ?? 0,
        price,
        free: !Object.values(price).some((v) => v),
        reasoning: false,
        kind: cfg.providers[m.provider]?.kind ?? "",
        fromConfig: true,
      };
    });
  for (const p of catalog.providers()) {
    if (!providerReady(p)) continue;
    for (const md of live.cached(p.id) ?? p.models) {
      const alias = `${p.id}/${md.id}`;
      if (out.some((m) => m.alias === alias)) continue;
      const price = priceObj(md.price);
      out.push({
        alias: `${p.id}/${md.id}`,
        provider: p.id,
        providerName: p.name,
        id: md.id,
        name: md.name,
        context: md.context,
        price,
        free: !md.price.some((v) => v),
        reasoning: md.reasoning,
        kind: p.kind,
        fromConfig: false,
      });
    }
  }
  return out;
}

/** Write `default_model = "<alias>"` at the top level of config.toml —
 * replacing the existing key if present, else inserting before any table. */
export function setDefaultModel(alias: string, home = proto.cliHome()): void {
  setTopLevel(home, "default_model", alias.replace(/["\\]/g, ""));
}

/** `user_name = "…"` — the greeting in the header. */
export function setUserName(name: string, home = proto.cliHome()): void {
  const clean = [...name].filter((c) => c !== "\0" && c >= " ").join("").trim();
  if (!clean) return;
  setTopLevel(home, "user_name", clean);
}

function setTopLevel(home: string, key: string, value: string): void {
  const file = path.join(home, "config.toml");
  let text = "";
  try {
    text = fs.readFileSync(file, "utf8");
  } catch {}
  const line = `${key} = ${tomlQuote(value)}`;
  const out: string[] = [];
  let done = false;
  let inTable = false;
  for (const l of text.split(/\r?\n/)) {
    const t = l.trimStart();
    if (t.startsWith("[")) inTable = true;
    if (!inTable && !done && new RegExp(`^${key}\\s*=`).test(t)) {
      out.push(line);
      done = true;
    } else out.push(l);
  }
  while (out.length && out[out.length - 1] === "") out.pop();
  if (!done) out.unshift(line);
  fs.mkdirSync(home, { recursive: true });
  fs.writeFileSync(file, out.join("\n") + "\n", "utf8");
}

/** AGENTS.md (Codex, Devin, pi), CLAUDE.md (Claude Code) and
 * .zcli/instructions.md from the workspace root, each capped at 32 KB. */
export function projectInstructions(workspace: string): string | null {
  const CAP = 32 * 1024;
  const parts: string[] = [];
  for (const name of ["AGENTS.md", "CLAUDE.md", ".zcli/instructions.md"]) {
    let text: string;
    try {
      text = fs.readFileSync(path.join(workspace, name), "utf8").trim();
    } catch {
      continue;
    }
    if (!text) continue;
    const body = text.length > CAP ? text.slice(0, CAP) : text;
    parts.push(`<project-instructions file="${name}">\n${body}\n</project-instructions>`);
  }
  return parts.length ? `The repository provides these instructions; follow them.\n\n${parts.join("\n\n")}` : null;
}

export type Message = {
  role: string;
  content: Record<string, any>[];
};

export class Session {
  home: string;
  id: string;
  harness: string;
  file: string;
  leaf: string | null = null;

  constructor(home: string, sid: string, harness: string) {
    this.home = home;
    this.id = sid;
    this.harness = harness;
    this.file = path.join(home, "sessions", `${sid}.jsonl`);
  }

  static create(home: string, harness: string): Session {
    fs.mkdirSync(path.join(home, "sessions"), { recursive: true });
    return new Session(home, proto.newId(), harness);
  }

  static open(home: string, sid: string): Session {
    const s = new Session(home, sid, "");
    if (!fs.existsSync(s.file)) throw new Error(`no session ${sid}`);
    const evs = s.events();
    s.leaf = evs.length ? evs[evs.length - 1].id : null;
    s.harness = evs.length ? evs[0].harness : "";
    return s;
  }

  events(): proto.Event[] {
    let text: string;
    try {
      text = fs.readFileSync(this.file, "utf8");
    } catch {
      return [];
    }
    return text
      .split(/\r?\n/)
      .filter((l) => l.trim())
      .map((l) => JSON.parse(l));
  }

  append(kind: string, data: Record<string, any>): proto.Event {
    const ev = proto.event(this.id, this.harness, this.leaf, kind, data);
    fs.appendFileSync(this.file, proto.toLine(ev) + "\n", "utf8");
    this.leaf = ev.id;
    return ev;
  }

  branch(at: string): proto.Event {
    this.leaf = at;
    return this.append("checkout", { at });
  }

  chain(): proto.Event[] {
    const evs = this.events();
    const byId = new Map(evs.map((e) => [e.id, e]));
    const out: proto.Event[] = [];
    let cur = evs.length ? evs[evs.length - 1] : undefined;
    while (cur) {
      out.push(cur);
      cur = cur.parent ? byId.get(cur.parent) : undefined;
    }
    out.reverse();
    return out;
  }

  replay(): Message[] {
    // Parent-chain → provider messages [{role, content:[blocks]}].
    const msgs: Message[] = [];
    for (const e of this.chain()) {
      const k = e.kind;
      const d = e.data ?? {};
      if (k === "user" && d.text) {
        msgs.push({
          role: "user",
          content: [{ type: "text", text: d.text }],
        });
      } else if (k === "assistant") {
        const blocks: Record<string, any>[] = [];
        if (d.text) blocks.push({ type: "text", text: d.text });
        for (const c of d.tool_calls ?? []) {
          blocks.push({
            type: "tool_use",
            id: c.id ?? "",
            name: c.name ?? "",
            input: c.input ?? {},
          });
        }
        if (blocks.length) msgs.push({ role: "assistant", content: blocks });
      } else if (k === "tool_result") {
        msgs.push({
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: d.tool_use_id ?? "",
              content: d.content ?? "",
              is_error: d.is_error ?? false,
            },
          ],
        });
      }
    }
    return msgs;
  }

  static list(home: string): Record<string, any>[] {
    const out: Record<string, any>[] = [];
    const d = path.join(home, "sessions");
    if (!fs.existsSync(d) || !fs.statSync(d).isDirectory()) return out;
    const files = fs
      .readdirSync(d)
      .filter((f) => f.endsWith(".jsonl"))
      .sort();
    for (const f of files) {
      const text = fs.readFileSync(path.join(d, f), "utf8");
      const evs: proto.Event[] = text
        .split(/\r?\n/)
        .filter((l) => l.trim())
        .map((l) => JSON.parse(l));
      if (!evs.length) continue;
      const firstUser =
        evs.find((e) => e.kind === "user")?.data?.text ?? "";
      out.push({
        id: f.replace(/\.jsonl$/, ""),
        harness: evs[0].harness,
        started: evs[0].ts,
        events: evs.length,
        preview: firstUser.slice(0, 80),
      });
    }
    out.sort((a, b) => (a.started < b.started ? -1 : a.started > b.started ? 1 : 0));
    return out;
  }

  static export(home: string, sid: string, fmt = "md"): string {
    const f = path.join(home, "sessions", `${sid}.jsonl`);
    const text = fs.readFileSync(f, "utf8");
    if (fmt === "jsonl") return text;
    const evs: proto.Event[] = text
      .split(/\r?\n/)
      .filter((l) => l.trim())
      .map((l) => JSON.parse(l));
    let md = `# session ${sid}\n\n`;
    for (const e of evs) {
      const d = e.data ?? {};
      if (e.kind === "user") {
        md += `## user · ${e.ts}\n\n${d.text ?? ""}\n\n`;
      } else if (e.kind === "assistant") {
        md += `## assistant · ${e.ts}\n\n${d.text ?? ""}\n`;
        for (const c of d.tool_calls ?? []) {
          md += `\n- \`${c.name ?? ""}\` ${JSON.stringify(c.input ?? {})}\n`;
        }
        md += "\n";
      } else if (e.kind === "tool_result") {
        md += `_${d.name ?? "tool"}_ → \`${String(d.content ?? "").slice(0, 200)}\`\n\n`;
      }
    }
    return md;
  }
}
