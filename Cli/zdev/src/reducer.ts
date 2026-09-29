// Pure reducer: session/update payload → transcript State. Kept pure so
// `bun test` can drive it without Ink or a child process.
import type { Item, State } from "./types";

let seq = 0;
const nid = () => `i${++seq}`;

/** Claude Code display names for tool rows (contract names are lowercase). */
export function toolLabel(name: string): string {
  return (
    {
      read: "Read",
      edit: "Update",
      write: "Write",
      bash: "Bash",
      grep: "Grep",
      glob: "Glob",
      ls: "List",
      memory_search: "MemorySearch",
      memory_write: "MemoryWrite",
      task: "Task",
      request_escalation: "Escalate",
      submit_plan: "SubmitPlan",
      tool_search: "ToolSearch",
      mcp_call: "MCP",
    }[name] ?? name
  );
}

/** Short argument summary for the `⏺ tool(arg)` line. */
export function summarizeArg(name: string, input: unknown): string | undefined {
  if (!input || typeof input !== "object") return undefined;
  const inp = input as Record<string, unknown>;
  const pick = (...keys: string[]) => {
    for (const k of keys) {
      const v = inp[k];
      if (typeof v === "string" && v) return v;
    }
    return undefined;
  };
  let v: string | undefined;
  switch (name) {
    case "bash":
      v = pick("command");
      break;
    case "read":
    case "write":
    case "edit":
      v = pick("path");
      break;
    case "grep":
      v = pick("pattern", "query");
      break;
    case "glob":
      v = pick("pattern", "path");
      break;
    case "ls":
      v = pick("path");
      break;
    case "memory_search":
    case "tool_search":
      v = pick("query");
      break;
    case "memory_write":
      v = pick("text");
      break;
    case "task":
      v = pick("prompt");
      break;
    case "submit_plan":
      v = pick("plan");
      break;
    default:
      v = pick("command", "path", "query", "pattern", "prompt", "text");
  }
  if (v === undefined) return undefined;
  v = v.replace(/\s+/g, " ").trim();
  return v.length > 90 ? v.slice(0, 89) + "…" : v;
}

function textOfContent(content: unknown): string {
  if (!Array.isArray(content)) return "";
  return content
    .map((c: any) =>
      c?.type === "content" && c?.content?.type === "text"
        ? c.content.text
        : c?.type === "text"
          ? c.text
          : "",
    )
    .filter(Boolean)
    .join("\n");
}

/** Apply one session/update payload. Returns a new State (does not mutate). */
export function applyUpdate(state: State, u: any): State {
  if (!u || typeof u !== "object") return state;
  const items = state.items.slice();
  switch (u.sessionUpdate) {
    case "agent_message_chunk": {
      const t = u.content?.text ?? "";
      if (!t) return state;
      const last = items[items.length - 1];
      if (last?.kind === "assistant") {
        items[items.length - 1] = { ...last, text: last.text + t };
      } else {
        items.push({ kind: "assistant", id: nid(), text: t });
      }
      return { ...state, items };
    }
    case "tool_call": {
      items.push({
        kind: "tool",
        id: nid(),
        toolCallId: u.toolCallId ?? nid(),
        name: u.title ?? "tool",
        arg: summarizeArg(u.title ?? "", u.input),
        status: "pending",
        input: u.input && typeof u.input === "object" ? u.input : undefined,
      });
      return { ...state, items };
    }
    case "tool_call_update": {
      const idx = items.findIndex(
        (i) => i.kind === "tool" && i.toolCallId === u.toolCallId,
      );
      const detail = textOfContent(u.content);
      if (idx === -1) {
        // defensive: update for a call we never saw
        items.push({
          kind: "tool",
          id: nid(),
          toolCallId: u.toolCallId ?? nid(),
          name: u.title ?? "tool",
          status: u.status === "failed" ? "failed" : "completed",
          detail,
        });
      } else {
        const it = items[idx] as Extract<Item, { kind: "tool" }>;
        items[idx] = {
          ...it,
          status: u.status === "failed" ? "failed" : "completed",
          detail: detail || it.detail,
        };
      }
      return { ...state, items };
    }
    case "usage": {
      const input = u.input ?? 0;
      const output = u.output ?? 0;
      const cacheRead = u.cache_read ?? 0;
      const cacheWrite = u.cache_write ?? 0;
      const model = String(u.model ?? "");
      const turnUsd = typeof u.turn_usd === "number" ? u.turn_usd : (u.usd ?? 0);
      const models = { ...state.usage.models };
      if (model && Number.isFinite(turnUsd)) models[model] = (models[model] ?? 0) + turnUsd;
      return {
        ...state,
        usage: {
          ...state.usage,
          input: state.usage.input + input,
          output: state.usage.output + output,
          cache_read: state.usage.cache_read + cacheRead,
          cache_write: state.usage.cache_write + cacheWrite,
          usd: u.usd ?? null,
          cum_usd: u.cum_usd ?? null,
          turns: state.usage.turns + 1,
          models,
        },
      };
    }
    case "plan": {
      items.push({ kind: "plan", id: nid(), text: u.plan ?? "" });
      return { ...state, items };
    }
    case "mode": {
      items.push({
        kind: "note",
        id: nid(),
        text: u.from ? `mode: ${u.from} → ${u.mode}` : `mode: ${u.mode}`,
      });
      return { ...state, items };
    }
    case "notice": {
      items.push({ kind: "note", id: nid(), text: u.text ?? "" });
      return { ...state, items };
    }
    case "error": {
      items.push({ kind: "note", id: nid(), text: u.text ?? "error", tone: "error" });
      return { ...state, items };
    }
    default:
      return state;
  }
}

/**
 * Split streaming markdown at the last paragraph break that isn't inside a
 * code fence: [finished part, still-growing tail]. The finished part can go
 * to scrollback (Ink <Static>) while the tail keeps streaming, so the live
 * region stays small no matter how long the reply gets.
 */
export function splitStreaming(text: string): [string, string] {
  const lines = text.split("\n");
  let inFence = false;
  let cut = -1; // line index of the last safe blank line
  for (let i = 0; i < lines.length - 1; i++) {
    if (/^\s*(```|~~~)/.test(lines[i])) inFence = !inFence;
    else if (!inFence && lines[i].trim() === "" && i > 0) cut = i;
  }
  if (cut <= 0) return ["", text];
  const head = lines.slice(0, cut).join("\n").replace(/\s+$/, "");
  const tail = lines.slice(cut + 1).join("\n");
  return head ? [head, tail] : ["", text];
}

let splitSeq = 0;

/**
 * Move the settled prefix of `live` into scrollback. Finished items settle;
 * a pending tool call holds everything after it back; the reply still
 * streaming (the last assistant item while `busy`) gives up its finished
 * paragraphs and keeps only its tail live.
 */
export function settle(live: Item[], busy: boolean): { done: Item[]; live: Item[] } {
  const done: Item[] = [];
  let i = 0;
  for (; i < live.length; i++) {
    const it = live[i];
    if (it.kind === "tool" && it.status === "pending") break;
    if (it.kind === "shell" && it.pending) break;
    if (it.kind === "assistant" && busy && i === live.length - 1) {
      const [head, tail] = splitStreaming(it.text);
      if (head) {
        done.push({ kind: "assistant", id: `${it.id}.${++splitSeq}`, text: head });
        return { done, live: [{ ...it, text: tail }] };
      }
      break;
    }
    done.push(it);
  }
  return { done, live: live.slice(i) };
}

/**
 * What a still-streaming reply shows in the live region: its last `rows`
 * terminal rows (wrapped at `width`), so the live region never outgrows the
 * terminal — Ink can't erase what has scrolled off. When the cut lands
 * inside a code fence, the fence is re-opened so highlighting stays right.
 * The whole reply still commits to scrollback once it settles.
 */
export function tailView(text: string, rows: number, width: number): { hidden: number; text: string } {
  const lines = text.split("\n");
  const w = Math.max(10, width);
  let used = 0;
  let start = lines.length;
  while (start > 0) {
    const need = Math.max(1, Math.ceil(Bun.stringWidth(lines[start - 1]) / w));
    if (used + need > rows) break;
    used += need;
    start--;
  }
  if (start === 0) return { hidden: 0, text };
  let fence: string | null = null;
  for (let i = 0; i < start; i++) {
    const m = lines[i].match(/^\s*(```|~~~)\s*([\w+#.-]*)/);
    if (m) fence = fence === null ? lines[i].trim() : null;
  }
  const shown = lines.slice(start);
  return { hidden: start, text: (fence !== null ? fence + "\n" : "") + shown.join("\n") };
}
