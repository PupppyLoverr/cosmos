// Windows Credential Manager through bun:ffi — the same generic credentials
// zcli's `keyring` crate writes, so a key saved by either CLI works in both:
//   TargetName  "<provider>.cosmos-cli"   (keyring: "{user}.{service}")
//   UserName    "<provider>"
//   blob        the key as UTF-16LE, no terminator
//   Persist     CRED_PERSIST_ENTERPRISE, Comment "keyring v3.6.3"
// Every call degrades to null/false off Windows or when advapi32 refuses.

const SERVICE = "cosmos-cli";
const CRED_TYPE_GENERIC = 1;
const CRED_PERSIST_ENTERPRISE = 3;
// CREDENTIALW on x64: see wincred.h — 80 bytes with natural alignment
const OFF = {
  flags: 0,
  type: 4,
  target: 8,
  comment: 16,
  lastWritten: 24,
  blobSize: 32,
  blob: 40,
  persist: 48,
  attrCount: 52,
  attrs: 56,
  alias: 64,
  user: 72,
  size: 80,
};

type Api = {
  CredReadW: (target: number, type: number, flags: number, out: number) => number;
  CredWriteW: (cred: number, flags: number) => number;
  CredDeleteW: (target: number, type: number, flags: number) => number;
  CredFree: (p: number) => void;
  ffi: typeof import("bun:ffi");
};

let api: Api | null | undefined;

function load(): Api | null {
  if (api !== undefined) return api;
  api = null;
  if (process.platform !== "win32" || process.arch !== "x64") return api;
  try {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const ffi = require("bun:ffi") as typeof import("bun:ffi");
    const { FFIType } = ffi;
    const lib = ffi.dlopen("advapi32.dll", {
      CredReadW: { args: [FFIType.ptr, FFIType.u32, FFIType.u32, FFIType.ptr], returns: FFIType.i32 },
      CredWriteW: { args: [FFIType.ptr, FFIType.u32], returns: FFIType.i32 },
      CredDeleteW: { args: [FFIType.ptr, FFIType.u32, FFIType.u32], returns: FFIType.i32 },
      CredFree: { args: [FFIType.ptr], returns: FFIType.void },
    });
    api = { ...(lib.symbols as any), ffi };
  } catch {
    api = null;
  }
  return api ?? null;
}

/** NUL-terminated UTF-16LE for a Win32 LPWSTR. */
function wstr(s: string): Uint16Array {
  const a = new Uint16Array(s.length + 1);
  for (let i = 0; i < s.length; i++) a[i] = s.charCodeAt(i);
  return a;
}

const target = (id: string) => `${id}.${SERVICE}`;

export function available(): boolean {
  return !!load();
}

/** The key stored for provider `id`, or null. */
export function read(id: string): string | null {
  const a = load();
  if (!a) return null;
  const { ptr, read: rd, toArrayBuffer } = a.ffi;
  const t = wstr(target(id));
  const out = new BigUint64Array(1);
  try {
    if (!a.CredReadW(ptr(t), CRED_TYPE_GENERIC, 0, ptr(out))) return null;
    const p = Number(out[0]);
    try {
      const size = rd.u32(p as any, OFF.blobSize);
      const blob = rd.ptr(p as any, OFF.blob);
      if (!size || !blob || size % 2) return null;
      const bytes = new Uint8Array(toArrayBuffer(blob as any, 0, size).slice(0));
      let s = "";
      for (let i = 0; i < bytes.length; i += 2) s += String.fromCharCode(bytes[i] | (bytes[i + 1] << 8));
      return s || null;
    } finally {
      a.CredFree(p);
    }
  } catch {
    return null;
  }
}

/** Save `key` for provider `id`; false when the store refused. */
export function write(id: string, key: string): boolean {
  const a = load();
  if (!a) return false;
  const { ptr } = a.ffi;
  try {
    const t = wstr(target(id));
    const user = wstr(id);
    const comment = wstr("keyring v3.6.3");
    const alias = wstr("");
    const blob = new Uint8Array(key.length * 2);
    for (let i = 0; i < key.length; i++) {
      const c = key.charCodeAt(i);
      blob[i * 2] = c & 0xff;
      blob[i * 2 + 1] = c >> 8;
    }
    const cred = new ArrayBuffer(OFF.size);
    const dv = new DataView(cred);
    const setPtr = (off: number, p: number) => dv.setBigUint64(off, BigInt(p), true);
    dv.setUint32(OFF.flags, 0, true);
    dv.setUint32(OFF.type, CRED_TYPE_GENERIC, true);
    setPtr(OFF.target, ptr(t));
    setPtr(OFF.comment, ptr(comment));
    dv.setUint32(OFF.blobSize, blob.length, true);
    setPtr(OFF.blob, ptr(blob));
    dv.setUint32(OFF.persist, CRED_PERSIST_ENTERPRISE, true);
    dv.setUint32(OFF.attrCount, 0, true);
    setPtr(OFF.attrs, 0);
    setPtr(OFF.alias, ptr(alias));
    setPtr(OFF.user, ptr(user));
    const ok = a.CredWriteW(ptr(new Uint8Array(cred)), 0) !== 0;
    blob.fill(0);
    return ok;
  } catch {
    return false;
  }
}

export function remove(id: string): void {
  const a = load();
  if (!a) return;
  try {
    a.CredDeleteW(a.ffi.ptr(wstr(target(id))), CRED_TYPE_GENERIC, 0);
  } catch {}
}
