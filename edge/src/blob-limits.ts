export const MAX_TOOL_BLOB_BYTES = 1024 * 1024;
export const MAX_ATTACHMENT_BLOB_BYTES = 32 * 1024 * 1024;

export const blobByteLimit = (partId: string): number =>
  partId.startsWith("att~") ? MAX_ATTACHMENT_BLOB_BYTES : MAX_TOOL_BLOB_BYTES;

export const isBlobTooLarge = (partId: string, byteLength: number): boolean =>
  byteLength > blobByteLimit(partId);
