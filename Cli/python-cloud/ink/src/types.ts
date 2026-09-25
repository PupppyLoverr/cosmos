// Transcript model — the reducer turns session/update payloads into Items.
export type ToolStatus = "pending" | "completed" | "failed";

export type Item =
  | { kind: "user"; id: string; text: string }
  | { kind: "assistant"; id: string; text: string }
  | {
      kind: "tool";
      id: string;
      toolCallId: string;
      name: string;
      arg?: string;
      status: ToolStatus;
      detail?: string;
      input?: Record<string, unknown>;
    }
  | { kind: "plan"; id: string; text: string }
  | { kind: "note"; id: string; text: string };

export type Usage = {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  usd: number | null;
  cum_usd: number | null;
};

export type Model = {
  alias: string;
  provider: string;
  id: string;
  context: number;
  free: boolean;
};

export type PermissionReq = {
  id: number; // JSON-RPC id to reply to
  toolCallId: string;
  tool: string;
  input: Record<string, unknown>;
  options: { id: string; name: string }[];
};

export type State = { items: Item[]; usage: Usage };
export const emptyState = (): State => ({
  items: [],
  usage: { input: 0, output: 0, cache_read: 0, cache_write: 0, usd: null, cum_usd: null },
});
