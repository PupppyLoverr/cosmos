import { EdgeClient } from "./edge";
import {
  ControlRpcMux,
  deviceRoomWsUrl,
  decodeFrame,
  ECHO_HEADER,
  encodeFrame,
  queueInterruptCommand,
  queueRespondInputCommand,
  queueRunCommand,
  RELAY_KIND,
} from "./relay";
import { pairingParams } from "./computer";
import { currentBearer } from "./auth";
import { bytesToBase64, chunkRanges, withAttachmentLines } from "./upload";
import type { Connection, StagedAttachment } from "./types";

/** Talks to a specific desktop host over the device-room relay. */
export class HostSession {
  constructor(
    readonly conn: Connection,
    readonly hostDeviceId: string,
    readonly pairingToken: string | null,
  ) {}

  private mux = new ControlRpcMux();
  private ws: WebSocket | null = null;
  private liveCancel: (() => void) | null = null;

  async connect(): Promise<void> {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) return;
    const token = await currentBearer(this.conn);
    const url = deviceRoomWsUrl(
      this.conn.edgeUrl,
      this.hostDeviceId,
      token,
      crypto.randomUUID(),
    );
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    await new Promise<void>((resolve, reject) => {
      const t = setTimeout(() => reject(new Error("relay connect timeout")), 12_000);
      ws.onopen = () => {
        clearTimeout(t);
        ws.send(encodeFrame(ECHO_HEADER, new Uint8Array()));
        resolve();
      };
      ws.onerror = () => {
        clearTimeout(t);
        reject(new Error("The device is offline"));
      };
    });
    ws.onmessage = (ev) => {
      const buf =
        ev.data instanceof ArrayBuffer
          ? new Uint8Array(ev.data)
          : null;
      if (!buf) return;
      const frame = decodeFrame(buf);
      if (!frame) return;
      if (frame.header.k === RELAY_KIND) {
        this.mux.failAll("The device is offline");
        return;
      }
      if (frame.header.k === "rpc") this.mux.handlePayload(frame.payload);
    };
  }

  private async call(method: string, params: unknown, timeoutMs = 30_000): Promise<unknown> {
    await this.connect();
    const { id, frame } = this.mux.request(method, params);
    const pending = this.mux.waitUnary(id, timeoutMs);
    this.ws?.send(frame);
    return pending;
  }

  private paired(extra: Record<string, unknown> = {}): Record<string, unknown> {
    if (!this.pairingToken) throw new Error("not paired");
    return pairingParams(this.conn, this.hostDeviceId, this.pairingToken, extra);
  }

  async sendRun(opts: {
    chatId: string;
    prompt: string;
    harness?: string | null;
    model?: string | null;
    reasoning?: string | null;
    cwd?: string | null;
    attachments?: string[];
  }): Promise<{ commandId: string }> {
    const messageId = crypto.randomUUID();
    const prompt = withAttachmentLines(opts.prompt, opts.attachments ?? []);
    const params = {
      ...queueRunCommand({ ...opts, prompt, messageId }),
      targetDeviceId: this.hostDeviceId,
    };
    const ok = (await this.call("QueueCommand", params)) as { commandId?: string };
    return { commandId: ok.commandId ?? messageId };
  }

  async stop(chatId: string): Promise<void> {
    await this.call("QueueCommand", {
      ...queueInterruptCommand(chatId),
      targetDeviceId: this.hostDeviceId,
    });
  }

  async respondInput(
    chatId: string,
    requestId: string,
    answers: Array<{ questionId: string; labels: string[] }>,
  ): Promise<void> {
    await this.call("QueueCommand", {
      ...queueRespondInputCommand(chatId, requestId, answers),
      targetDeviceId: this.hostDeviceId,
    });
  }

  async listHarnesses(): Promise<unknown> {
    return this.call("ListHarnesses", { targetDeviceId: this.hostDeviceId });
  }

  async listModels(harness?: string): Promise<unknown> {
    return this.call("ListModels", {
      targetDeviceId: this.hostDeviceId,
      harness,
    });
  }

  async mutate(op: Record<string, unknown>): Promise<unknown> {
    return this.call("Mutate", { ...op, targetDeviceId: this.hostDeviceId });
  }

  async computerStatus(): Promise<unknown> {
    return this.call(
      "ComputerStatus",
      pairingParams(this.conn, this.hostDeviceId, this.pairingToken ?? ""),
    );
  }

  async requestTrust(): Promise<unknown> {
    const token = this.pairingToken;
    if (!token) throw new Error("missing pairing token");
    return this.call("ComputerRequestTrust", {
      targetDeviceId: this.hostDeviceId,
      callerDeviceId: this.conn.deviceId,
      callerName: this.conn.deviceName,
      platform: "android",
      pairingToken: token,
    });
  }

  async computerStop(reason = "stopped from phone"): Promise<unknown> {
    return this.call("ComputerStop", this.paired({ reason }));
  }

  async computerDecide(
    decision: "approve" | "deny" | "pause" | "stop",
    reason?: string,
  ): Promise<unknown> {
    return this.call("ComputerDecide", this.paired({ decision, reason }));
  }

  async computerTool(name: string, args: Record<string, unknown>): Promise<unknown> {
    return this.call(
      "ComputerToolCall",
      this.paired({ name, arguments: args }),
      90_000,
    );
  }

  async listTools(): Promise<unknown> {
    return this.call("ListTools", { targetDeviceId: this.hostDeviceId });
  }

  async connectComposio(apiKey: string): Promise<unknown> {
    return this.call("ConnectComposio", {
      targetDeviceId: this.hostDeviceId,
      apiKey,
    });
  }

  /** Upload bytes to the host uploads jail, then return the committed path. */
  async uploadAttachment(att: StagedAttachment): Promise<{ path: string; uploadId: string }> {
    const uploadId = crypto.randomUUID();
    const ranges = chunkRanges(att.bytes.length);
    for (const range of ranges) {
      const slice = att.bytes.subarray(range.start, range.end);
      await this.call(
        "UploadChunk",
        {
          targetDeviceId: this.hostDeviceId,
          uploadId,
          seq: range.seq,
          data: bytesToBase64(slice),
        },
        90_000,
      );
    }
    const committed = (await this.call(
      "UploadCommit",
      {
        targetDeviceId: this.hostDeviceId,
        uploadId,
        fileName: att.name,
      },
      150_000,
    )) as { path?: string };
    if (!committed?.path) throw new Error("UploadCommit did not return a host path");
    return { path: committed.path, uploadId };
  }

  async readAttachmentDataUrl(path: string): Promise<string> {
    let offset = 0;
    let data = "";
    let mime = "image/png";
    for (;;) {
      const chunk = (await this.call(
        "ReadAttachmentChunk",
        { targetDeviceId: this.hostDeviceId, path, offset },
        30_000,
      )) as {
        data?: string;
        nextOffset?: number;
        done?: boolean;
        mimeType?: string;
      };
      data += chunk.data ?? "";
      if (chunk.mimeType) mime = chunk.mimeType;
      if (chunk.done) break;
      offset = chunk.nextOffset ?? offset + 45_000;
    }
    return `data:${mime};base64,${data}`;
  }
  async watchLive(onFrame: (item: unknown) => void): Promise<() => void> {
    this.stopLive();
    await this.connect();
    const { id, frame } = this.mux.request("WatchComputerLive", this.paired());
    const sub = this.mux.waitStream(id, onFrame, 30 * 60_000);
    this.ws?.send(frame);
    this.liveCancel = () => {
      sub.cancel();
      this.liveCancel = null;
    };
    void sub.promise.catch(() => undefined);
    return () => this.stopLive();
  }

  async watchEvents(onItem: (item: unknown) => void): Promise<() => void> {
    await this.connect();
    const { id, frame } = this.mux.request(
      "WatchComputerEvents",
      pairingParams(this.conn, this.hostDeviceId, this.pairingToken ?? ""),
    );
    const sub = this.mux.waitStream(id, onItem, 30 * 60_000);
    this.ws?.send(frame);
    return () => sub.cancel();
  }

  stopLive(): void {
    this.liveCancel?.();
    this.liveCancel = null;
  }

  close(): void {
    this.stopLive();
    this.mux.failAll("closed");
    this.ws?.close();
    this.ws = null;
  }
}

export { EdgeClient };
