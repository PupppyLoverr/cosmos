import * as FileSystem from "expo-file-system";
import type { StagedAttachment } from "./protocol/types";
import { parseShareUrl } from "./protocol/share";
import { bytesFromBase64 } from "./protocol/upload";

export { parseShareUrl };

export async function attachmentFromShareUrl(url: string): Promise<StagedAttachment | null> {
  const parsed = parseShareUrl(url);
  if (!parsed) return null;
  if (parsed.text && !parsed.uri) {
    const bytes = new TextEncoder().encode(parsed.text);
    return {
      name: "clipboard.txt",
      mime: "text/plain",
      bytes,
      source: "share",
    };
  }
  if (!parsed.uri) return null;
  const b64 = await FileSystem.readAsStringAsync(parsed.uri, {
    encoding: FileSystem.EncodingType.Base64,
  });
  const name = decodeURIComponent(parsed.uri.split("/").pop() || "shared.bin");
  const mime = name.match(/\.(png|jpe?g|webp|gif)$/i) ? "image/jpeg" : "application/octet-stream";
  return {
    name,
    mime,
    bytes: bytesFromBase64(b64),
    source: "share",
  };
}
