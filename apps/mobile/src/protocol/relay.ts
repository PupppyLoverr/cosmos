/** Device-room relay frame codec + ControlRpc client.
 *
 * Frame: uleb128(headerLen) ‖ UTF-8 JSON header ‖ payload
 * Header key order MUST be {"s","k"} for RPC (byte parity with iOS/Rust).
 * Clients never set to/from — the edge stamps from.
 */

export const RPC_KIND = "rpc";
export const RELAY_KIND = " relay";
export const ECHO_KIND = "echo";
export const NUDGE_KIND = "nudge";

export function encodeUleb128(value: number): Uint8Array {
  const bytes: number[] = [];
  let n = value >>> 0;
  do {
    let b = n & 0x7f;
    n >>>= 7;
    if (n !== 0) b |= 0x80;
    bytes.push(b);
  } while (n !== 0);
  return Uint8Array.from(bytes);
}

export function decodeUleb128(
  data: Uint8Array,
  offset = 0,
): { value: number; next: number } | null {
  let value = 0;
  let shift = 0;
  let i = offset;
  while (i < data.length) {
    const b = data[i++];
    value |= (b & 0x7f) << shift;
    if ((b & 0x80) === 0) return { value, next: i };
    shift += 7;
    if (shift > 35) return null;
  }
  return null;
}

export function encodeFrame(headerJson: string, payload: Uint8Array): Uint8Array {
  const header = new TextEncoder().encode(headerJson);
  const len = encodeUleb128(header.length);
  const out = new Uint8Array(len.length + header.length + payload.length);
  out.set(len, 0);
  out.set(header, len.length);
  out.set(payload, len.length + header.length);
  return out;
}

export function decodeFrame(
  data: Uint8Array,
): { header: { s?: string; k?: string }; payload: Uint8Array } | null {
  const len = decodeUleb128(data, 0);
  if (!len) return null;
  const end = len.next + len.value;
  if (end > data.length) return null;
  const headerText = new TextDecoder().decode(data.slice(len.next, end));
  let header: { s?: string; k?: string };
  try {
    header = JSON.parse(headerText);
  } catch {
    return null;
  }
  return { header, payload: data.slice(end) };
}

export const RPC_HEADER = '{"s":"rpc","k":"rpc"}';
export const ECHO_HEADER = '{"s":"echo","k":"echo"}';

export function deviceRoomWsUrl(
  edgeUrl: string,
  deviceId: string,
  token: string,
  connId: string,
): string {
  const ws = edgeUrl.replace(/^http/i, "ws").replace(/\/$/, "");
  const q = new URLSearchParams({
    role: "client",
    connId,
    token,
  });
  return `${ws}/device/${encodeURIComponent(deviceId)}/ws?${q.toString()}`;
}

export interface RpcReply {
  id: number;
  ok?: unknown;
  err?: string;
  item?: unknown;
  done?: boolean;
}

export type UnaryHandler = (reply: RpcReply) => void;
export type StreamHandler = (reply: RpcReply) => void;

/** Minimal ControlRpc client. Wire I/O is injected so tests don't need a socket. */
export class ControlRpcMux {
  private nextId = 1;
  private unary = new Map<number, UnaryHandler>();
  private streams = new Map<number, StreamHandler>();

  alloc(): number {
    const id = this.nextId++;
    return id;
  }

  waitUnary(id: number, timeoutMs = 30_000): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.unary.delete(id);
        reject(new Error("The device didn't respond"));
      }, timeoutMs);
      this.unary.set(id, (reply) => {
        clearTimeout(timer);
        if (reply.err) reject(new Error(reply.err));
        else resolve(reply.ok);
      });
    });
  }

  waitStream(
    id: number,
    onItem: (item: unknown) => void,
    timeoutMs = 120_000,
  ): { promise: Promise<void>; cancel: () => void } {
    let timer: ReturnType<typeof setTimeout>;
    let settled = false;
    const promise = new Promise<void>((resolve, reject) => {
      timer = setTimeout(() => {
        this.streams.delete(id);
        if (!settled) {
          settled = true;
          reject(new Error("The device didn't respond"));
        }
      }, timeoutMs);
      this.streams.set(id, (reply) => {
        if (reply.err) {
          clearTimeout(timer);
          this.streams.delete(id);
          if (!settled) {
            settled = true;
            reject(new Error(reply.err));
          }
          return;
        }
        if (reply.done) {
          clearTimeout(timer);
          this.streams.delete(id);
          if (!settled) {
            settled = true;
            resolve();
          }
          return;
        }
        if ("item" in reply) onItem(reply.item);
      });
    });
    return {
      promise,
      cancel: () => {
        clearTimeout(timer);
        this.streams.delete(id);
        if (!settled) {
          settled = true;
        }
      },
    };
  }

  handlePayload(bytes: Uint8Array): void {
    const text = new TextDecoder().decode(bytes);
    for (const line of text.split("\n")) {
      if (!line.trim()) continue;
      let obj: RpcReply;
      try {
        obj = JSON.parse(line);
      } catch {
        continue;
      }
      if (typeof obj.id !== "number") continue;
      const stream = this.streams.get(obj.id);
      if (stream) {
        stream(obj);
        continue;
      }
      const h = this.unary.get(obj.id);
      if (!h) continue;
      if (obj.done || obj.err || "ok" in obj) {
        this.unary.delete(obj.id);
        h(obj);
      }
    }
  }

  request(method: string, params: unknown): { id: number; frame: Uint8Array } {
    const id = this.alloc();
    const body = new TextEncoder().encode(
      JSON.stringify({ id, method, params }) + "\n",
    );
    return { id, frame: encodeFrame(RPC_HEADER, body) };
  }

  failAll(message: string): void {
    for (const [id, h] of this.unary) {
      h({ id, err: message });
    }
    this.unary.clear();
    for (const [id, h] of this.streams) {
      h({ id, err: message, done: true });
    }
    this.streams.clear();
  }
}

export function queueRunCommand(input: {
  chatId: string;
  prompt: string;
  harness?: string | null;
  model?: string | null;
  reasoning?: string | null;
  cwd?: string | null;
  attachments?: string[];
  messageId: string;
}): { chatId: string; command: unknown } {
  return {
    chatId: input.chatId,
    command: {
      kind: "run",
      request: {
        prompt: input.prompt,
        harness: input.harness ?? undefined,
        model: input.model ?? undefined,
        reasoning: input.reasoning ?? undefined,
        modelOptions: {},
        cwd: input.cwd ?? "",
        sandbox: "workspace-write",
        autoApprove: false,
        resume: null,
        attachments: input.attachments ?? [],
      },
      messageId: input.messageId,
    },
  };
}

export function queueInterruptCommand(chatId: string): {
  chatId: string;
  command: { kind: "interrupt" };
} {
  return { chatId, command: { kind: "interrupt" } };
}

export function queueRespondInputCommand(
  chatId: string,
  requestId: string,
  answers: Array<{ questionId: string; labels: string[] }>,
): { chatId: string; command: unknown } {
  return {
    chatId,
    command: {
      kind: "respondInput",
      requestId,
      answers,
    },
  };
}
