import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  applyDelta,
  devices,
  emptyStore,
  sessions,
  spaces,
  type RowsResponse,
} from "../src/protocol/registry.ts";
import { sessionIndicator } from "../src/protocol/types.ts";
import {
  decodeFrame,
  encodeFrame,
  encodeUleb128,
  decodeUleb128,
  queueInterruptCommand,
  queueRunCommand,
  queueRespondInputCommand,
  RPC_HEADER,
  ControlRpcMux,
} from "../src/protocol/relay.ts";
import { randomPairingToken } from "../src/protocol/computer.ts";
import { deliveryForPush } from "../src/protocol/notices.ts";
import { chunkRanges, UPLOAD_CHUNK_BYTES, withAttachmentLines } from "../src/protocol/upload.ts";

const here = dirname(fileURLToPath(import.meta.url));

describe("registry live payload", () => {
  const raw = readFileSync(
    join(here, "../__fixtures__/registry_rows_live.json"),
    "utf8",
  );
  const resp = JSON.parse(raw) as RowsResponse;
  const store = emptyStore();
  applyDelta(store, resp);

  it("parses devices, spaces, sessions from a real edge dump", () => {
    const d = devices(store);
    const sp = spaces(store);
    const chats = sessions(store);
    assert.equal(chats.length, 5);
    assert.equal(sp.length, 3);
    assert.ok(d.length >= 1);
    assert.equal(d[0].platform, "linux");
    assert.equal(d[0].name, "Desktop (Linux)");
    const titles = new Set(chats.map((c) => c.title));
    assert.ok(titles.has("Native Cosmos Rust Rewrite"));
    assert.ok(titles.has("Rebalance Player Stats Caps"));
    const rewrite = chats.find((c) => c.title === "Native Cosmos Rust Rewrite")!;
    assert.equal(rewrite.harness, "mock");
    assert.equal(rewrite.model, "fable-5");
    assert.equal(rewrite.branch, "cosmos/main");
    assert.ok(rewrite.deviceId);
  });

  it("does not invent sessions", () => {
    assert.equal(emptyStore().size, 0);
    assert.equal(sessions(emptyStore()).length, 0);
  });

  it("computes unseen/completed indicators from registry clocks", () => {
    const rewrite = sessions(store).find(
      (c) => c.title === "Native Cosmos Rust Rewrite",
    )!;
    const now = 1789650871758 + 1;
    assert.equal(sessionIndicator(rewrite, now), "completed");
  });
});

describe("device-room frame codec", () => {
  it("round-trips uleb128 + header + payload", () => {
    const payload = new TextEncoder().encode('{"id":1,"ok":{}}\n');
    const frame = encodeFrame(RPC_HEADER, payload);
    const decoded = decodeFrame(frame);
    assert.ok(decoded);
    assert.equal(decoded!.header.s, "rpc");
    assert.equal(decoded!.header.k, "rpc");
    assert.equal(new TextDecoder().decode(decoded!.payload), '{"id":1,"ok":{}}\n');
  });

  it("uleb128 matches iOS/Rust encoding for typical header sizes", () => {
    const n = RPC_HEADER.length;
    const encoded = encodeUleb128(n);
    const decoded = decodeUleb128(encoded);
    assert.equal(decoded?.value, n);
  });
});

describe("session control payloads", () => {
  it("QueueCommand run is tagged and carries a client message id", () => {
    const cmd = queueRunCommand({
      chatId: "c1",
      prompt: "hello",
      harness: "opencode",
      model: "opencode/mimo-v2.5-free",
      messageId: "m1",
    });
    assert.equal(cmd.chatId, "c1");
    const body = cmd.command as { kind: string; messageId: string; request: { prompt: string } };
    assert.equal(body.kind, "run");
    assert.equal(body.messageId, "m1");
    assert.equal(body.request.prompt, "hello");
  });

  it("stop is a real interrupt command, not a host nudge", () => {
    const cmd = queueInterruptCommand("c1");
    assert.deepEqual(cmd.command, { kind: "interrupt" });
  });

  it("respondInput is a QueueCommand, not a local-only UI toggle", () => {
    const cmd = queueRespondInputCommand("c1", "req", [{ questionId: "q", labels: ["yes"] }]);
    const body = cmd.command as { kind: string; requestId: string };
    assert.equal(body.kind, "respondInput");
    assert.equal(body.requestId, "req");
  });
});

describe("pairing + notices", () => {
  it("pairing tokens are unguessable (>=16 bytes hex)", () => {
    const t = randomPairingToken();
    assert.ok(t.length >= 32);
  });

  it("push without credentials is credential-missing, not success", () => {
    assert.equal(deliveryForPush(false), "credential-missing");
    assert.equal(deliveryForPush(true), "pushed");
  });
});

describe("uploads + streams", () => {
  it("chunks empty files as a single seq-0 slot", () => {
    const ranges = chunkRanges(0);
    assert.equal(ranges.length, 1);
    assert.equal(ranges[0].seq, 0);
  });

  it("splits large payloads on the desktop-compatible boundary", () => {
    const ranges = chunkRanges(UPLOAD_CHUNK_BYTES + 12);
    assert.equal(ranges.length, 2);
    assert.equal(ranges[0].end - ranges[0].start, UPLOAD_CHUNK_BYTES);
  });

  it("attachment lines ride the prompt so the host agent sees the files", () => {
    const text = withAttachmentLines("hi", ["/tmp/a.png"]);
    assert.match(text, /hi/);
    assert.match(text, /\/tmp\/a\.png/);
  });

  it("stream mux delivers items then done", async () => {
    const mux = new ControlRpcMux();
    const { id, frame } = mux.request("WatchComputerLive", {});
    assert.ok(frame.byteLength > 0);
    const items: unknown[] = [];
    const sub = mux.waitStream(id, (item) => items.push(item), 1000);
    mux.handlePayload(new TextEncoder().encode(JSON.stringify({ id, item: { n: 1 } }) + "\n"));
    mux.handlePayload(new TextEncoder().encode(JSON.stringify({ id, done: true }) + "\n"));
    await sub.promise;
    assert.deepEqual(items, [{ n: 1 }]);
  });
});
