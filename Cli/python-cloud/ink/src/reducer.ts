// Pure reducer: session/update payload → transcript State. Kept pure so
// `bun test` can drive it without Ink or a child process.
import type { Item, State } from "./types";

let seq = 0;
const nid = () => `i${++seq}`;

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
      return {
        ...state,
        usage: {
          input: u.input ?? 0,
          output: u.output ?? 0,
          cache_read: u.cache_read ?? 0,
          cache_write: u.cache_write ?? 0,
          usd: u.usd ?? null,
          cum_usd: u.cum_usd ?? null,
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
    default:
      return state;
  }
}

/** Commands for the slash palette; `q` is the typed prefix (no leading /). */
export const SLASH_COMMANDS = [
  { name: "/plan", desc: "Plan mode — read-only, ends in an approvable plan" },
  { name: "/agent", desc: "Agent mode — tools on, ask before risky calls" },
  { name: "/yolo", desc: "Yolo mode — never ask, just run" },
  { name: "/approve", desc: "Approve the submitted plan and run it" },
  { name: "/model", desc: "Pick a model alias for this session" },
  { name: "/session", desc: "Show the session id" },
  { name: "/usage", desc: "Show token usage and cost" },
  { name: "/help", desc: "List commands and keys" },
  { name: "/clear", desc: "Clear the transcript" },
  { name: "/quit", desc: "Quit zdev" },
];

export function filterSlash(
  query: string,
  commands = SLASH_COMMANDS,
): { name: string; desc: string }[] {
  const q = query.toLowerCase();
  return commands.filter((c) => c.name.slice(1).toLowerCase().startsWith(q));
}
