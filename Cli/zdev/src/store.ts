// Config (TOML) + session store — same semantics as zcore (contract §1, §6).
import fs from "node:fs";
import path from "node:path";
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

export function loadConfig(home: string): Config {
  const cfg: Config = {
    default_model: null,
    providers: {},
    models: {},
    permissions: { allow: [], ask: [] },
    subagents: { max_parallel: 3 },
    bridge: { large_read_lines: 200, summarizer_model: "" },
    mcp: {},
    tools: {},
    hooks: [],
  };
  const p = path.join(home, "config.toml");
  if (fs.existsSync(p) && fs.statSync(p).isFile()) {
    try {
      const loaded = toml.parse(fs.readFileSync(p, "utf8"));
      for (const [k, v] of Object.entries(loaded)) {
        const cur = (cfg as any)[k];
        if (
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
    cfg.default_model = "fixture";
  }
  return cfg;
}

export function allowedHosts(cfg: Config): string[] {
  const out: string[] = [];
  for (const p of Object.values(cfg.providers)) {
    const url = p?.base_url ?? "";
    if (url.includes("://")) {
      const host = url.split("://", 2)[1].split("/")[0].split(":")[0].split("?")[0];
      if (host) out.push(host);
    }
    // subscription kind talks to the OAuth issuer for token refresh too
    if (p?.kind === "chatgpt") out.push("auth.openai.com");
  }
  return out;
}

export function providerFor(
  cfg: Config,
  alias: string,
): [Record<string, any>, string] | null {
  const m = cfg.models[alias];
  if (!m) return null;
  const p = cfg.providers[m.provider];
  if (!p) return null;
  const price = m.price ?? {};
  return [
    {
      kind: p.kind ?? "fixture",
      base_url: p.base_url ?? "",
      api_key_env: p.api_key_env ?? "",
      model: m.id,
      context: m.context ?? 128000,
      prices: price,
      home: proto.cliHome(),
    },
    m.id,
  ];
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
