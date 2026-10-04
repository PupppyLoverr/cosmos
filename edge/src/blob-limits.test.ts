import { expect, it } from "vitest";
import {
  blobByteLimit,
  isBlobTooLarge,
  MAX_ATTACHMENT_BLOB_BYTES,
  MAX_TOOL_BLOB_BYTES
} from "./blob-limits";

it("uses the attachment cap only for attachment blob ids", () => {
  expect(blobByteLimit("att~12345678-photo.png")).toBe(MAX_ATTACHMENT_BLOB_BYTES);
  expect(blobByteLimit("tool-call_1")).toBe(MAX_TOOL_BLOB_BYTES);
  expect(MAX_ATTACHMENT_BLOB_BYTES).toBe(32 * 1024 * 1024);
  expect(MAX_TOOL_BLOB_BYTES).toBe(1024 * 1024);
  expect(isBlobTooLarge("att~12345678-photo.png", MAX_ATTACHMENT_BLOB_BYTES)).toBe(false);
  expect(isBlobTooLarge("att~12345678-photo.png", MAX_ATTACHMENT_BLOB_BYTES + 1)).toBe(true);
  expect(isBlobTooLarge("tool-call_1", MAX_TOOL_BLOB_BYTES)).toBe(false);
  expect(isBlobTooLarge("tool-call_1", MAX_TOOL_BLOB_BYTES + 1)).toBe(true);
});
