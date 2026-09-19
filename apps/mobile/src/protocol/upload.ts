/** Chunked UploadChunk / UploadCommit helpers — same sizing as desktop/iOS. */

/** Base64 chars per UploadChunk (attachments.rs UPLOAD_CHUNK_B64_CHARS). */
export const UPLOAD_CHUNK_B64_CHARS = 680_000;

/** Binary bytes per chunk: b64/4*3 so independent slices concatenate. */
export const UPLOAD_CHUNK_BYTES = (UPLOAD_CHUNK_B64_CHARS / 4) * 3;

function nodeBuffer():
  | { from: (input: string | Uint8Array, enc?: string) => { toString: (enc: string) => string } }
  | undefined {
  return (globalThis as { Buffer?: { from: (input: string | Uint8Array, enc?: string) => { toString: (enc: string) => string } } }).Buffer;
}

export function bytesFromBase64(b64: string): Uint8Array {
  const Buf = nodeBuffer();
  if (Buf) {
    return Uint8Array.from(Buf.from(b64, "base64") as unknown as Iterable<number>);
  }
  const binary = atob(b64);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

export function bytesToBase64(bytes: Uint8Array): string {
  const Buf = nodeBuffer();
  if (Buf) {
    return Buf.from(bytes).toString("base64");
  }
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function chunkRanges(byteLength: number): Array<{ seq: number; start: number; end: number }> {
  const ranges: Array<{ seq: number; start: number; end: number }> = [];
  let offset = 0;
  let seq = 0;
  do {
    const end = Math.min(offset + UPLOAD_CHUNK_BYTES, byteLength);
    ranges.push({ seq, start: offset, end });
    offset = end;
    seq += 1;
  } while (offset < byteLength);
  if (byteLength === 0) {
    return [{ seq: 0, start: 0, end: 0 }];
  }
  return ranges;
}

export function withAttachmentLines(prompt: string, paths: string[]): string {
  if (!paths.length) return prompt;
  const lines = paths.map((p) => `- ${p}`).join("\n");
  const trimmed = prompt.trim();
  return trimmed ? `${trimmed}\n\nAttached files (on the desktop):\n${lines}` : `Attached files (on the desktop):\n${lines}`;
}
