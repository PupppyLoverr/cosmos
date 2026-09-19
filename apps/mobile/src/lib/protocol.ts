// Wire types for the Cosmos engine RPC (`ws://<host>:27654` locally, or the
// device-room relay over `wss://<edge>/device/<id>/ws` remotely). Field names
// mirror crates/proto + crates/doc serde (camelCase unless noted).

export type HarnessId =
  | 'claude-code' | 'codex' | 'cursor' | 'devin' | 'grok' | 'hermes' | 'pi'
  | 'opencode' | 'antigravity' | 'amp' | 'kimi-code' | 'mock';

export type SandboxLevel = 'read-only' | 'workspace-write' | 'danger-full-access';
export type ReasoningLevel =
  | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max' | 'ultra'
  | 'ultracode' | 'ultrathink';

export interface ChatConfig {
  harness: HarnessId;
  model?: string | null;
  reasoning?: ReasoningLevel | null;
  modelOptions?: Record<string, unknown>;
  sandbox: SandboxLevel;
}

export interface Device {
  id: string;
  name: string;
  platform: string;
  lastSeenAt?: string | null;
  createdAt?: string | null;
  version?: string;
  capabilities?: string[];
}

export interface Space {
  id: string;
  deviceId: string;
  path: string;
  name?: string | null;
  gitDetected: boolean;
  createdAt: string;
}

export interface Chat {
  id: string;
  deviceId: string;
  title?: string | null;
  archived: boolean;
  cwd?: string | null;
  branch?: string | null;
  config?: ChatConfig | null;
  lastMessagePreview?: string | null;
  lastMessageAt?: string | null;
  createdAt: string;
  harnessSessionId?: string | null;
  spaceId?: string | null;
  lastSeenAt?: string | null;
  roomGen?: number;
}

export type SessionStatus = 'idle' | 'working' | 'awaitingInput' | 'errored';

export interface Session {
  chatId: string;
  deviceId: string;
  status: SessionStatus;
  startedAt?: string | null;
  updatedAt: string;
  lastCompletedTurn?: string | null;
}

export type ChatIndicator = 'working' | 'awaitingInput' | 'errored' | 'completed' | 'idle';

// -- transcript (crates/doc) ------------------------------------------------

export type MessageRole = 'user' | 'assistant' | 'system';
export type MessageStatus = 'streaming' | 'complete' | 'aborted';
export type SubagentStatus = 'running' | 'done' | 'failed';

// Wire-tagged union mirroring crates/proto::agent::ToolCall (kind-tagged).
export type ToolCall =
  | { kind: 'exec'; command: string }
  | { kind: 'readFile'; path: string }
  | { kind: 'writeFile'; path: string }
  | { kind: 'editFile'; path: string }
  | { kind: 'applyPatch'; path?: string | null }
  | { kind: 'search'; pattern: string; path?: string | null }
  | { kind: 'glob'; pattern: string }
  | { kind: 'webFetch'; url: string; prompt?: string | null }
  | { kind: 'webSearch'; query: string }
  | { kind: 'todo'; items?: { text?: string; status?: string }[] }
  | { kind: 'mcp'; server: string; tool: string; input?: unknown }
  | { kind: 'unknown'; name: string; input?: unknown };

/** Display name for a tool chip: mcp → the tool name, unknown → name, else kind. */
export function toolCallName(call: ToolCall): string {
  if (call.kind === 'mcp') return call.tool;
  if (call.kind === 'unknown') return call.name.replace(/^cosmos-computer_/, '');
  return call.kind;
}

/** Whether this call is a Cosmos Computer action (drives the Watch UI).
 *  Harnesses report MCP calls either tagged (`mcp`, server `cosmos-computer`)
 *  or untagged (`unknown`, name `cosmos-computer_computer_*`). */
export function isComputerCall(call: ToolCall): boolean {
  if (call.kind === 'mcp' && call.server === 'cosmos-computer') return true;
  const name = toolCallName(call);
  return name.startsWith('computer_') || name.startsWith('cosmos-computer_computer_');
}

export interface UserInputQuestion {
  id: string;
  header: string;
  question: string;
  options: string[];
  multiSelect?: boolean;
}

export interface UserInputAnswer {
  questionId: string;
  labels: string[];
}

export type MessagePart =
  | { kind: 'text'; id: string; text: string }
  | { kind: 'reasoning'; id: string; text: string }
  | { kind: 'image'; id: string; path: string; name: string; mimeType: string }
  | { kind: 'error'; id: string; message: string }
  | {
      kind: 'tool'; id: string; call: ToolCall; isError?: boolean; resolved?: boolean;
      output?: string | null; outputRef?: string; outputBytes?: number;
      subagentRef?: string; subagentStatus?: SubagentStatus; subagentTail?: string;
      diffStats?: { path: string; additions: number; deletions: number }[];
    }
  | { kind: 'input'; id: string; requestId: string; questions: UserInputQuestion[]; resolved?: boolean };

export interface SessionMessageEntry {
  id: string;
  role: MessageRole;
  parts: MessagePart[];
  createdAt: number;
  deviceId: string;
  status?: MessageStatus | null;
  continuationOf?: string | null;
}

export type TranscriptFrame =
  | { reset: SessionMessageEntry[] }
  | {
      upsert?: { after: string | null; entry: SessionMessageEntry }[];
      append?: { entry: string; part: string; text: string }[];
      remove?: string[];
      count: number;
    };

export interface TranscriptUpdate {
  reset?: SessionMessageEntry[];
  upsert?: { after: string | null; entry: SessionMessageEntry }[];
  append?: { entry: string; part: string; text: string }[];
  remove?: string[];
  count?: number;
  contextUsage?: { tokens?: number; limit?: number } | null;
}

// -- commands (doc command ledger) -------------------------------------------

export interface RunRequest {
  prompt: string;
  harness?: HarnessId;
  model?: string | null;
  reasoning?: ReasoningLevel | null;
  modelOptions?: Record<string, unknown>;
  cwd: string;
  sandbox: SandboxLevel;
  autoApprove?: boolean;
  resume?: string | null;
  attachments?: string[];
}

export type SessionCommandPayload =
  | { kind: 'run'; request: RunRequest; messageId: string }
  | { kind: 'steer'; prompt: string; messageId?: string }
  | { kind: 'interrupt' }
  | { kind: 'respondInput'; requestId: string; answers: UserInputAnswer[] };

// -- misc -------------------------------------------------------------------

export interface HarnessInfo {
  id: HarnessId;
  name?: string;
  enabled?: boolean;
}

export interface EngineInfo {
  version?: string;
  deviceId?: string;
  [k: string]: unknown;
}

export interface ComputerStatus {
  state?: string;
  control?: string;
  [k: string]: unknown;
}

export interface ComputerObservation {
  screenshotBase64?: string;
  screenshot?: string;
  pngBase64?: string;
  imageBase64?: string;
  text?: string;
  [k: string]: unknown;
}
