# Local sister-app discovery

After successfully binding its loopback IPC listener, headed and headless Cosmos
best-effort publish `~/Library/Application Support/Cosmos/handshake.json` on macOS.
Other hosts use `~/.config/cosmos/handshake.json`. This does not change RPC
authorization, workspace boundaries, provider storage, or subscription behavior.

Version 1 contains `app: "cosmos"`, the loopback `rpc` address, the current
executable `bin`, RFC3339 `updated_at`, and `capabilities: ["tools_reference"]`.
`tools_key_ref` is `file:<absolute device data root>/tools.json#provider=composio`.
The root is the engine's configured `data_dir` (normally `~/.zeron`, overridden
by `ZERON_DATA_DIR`), not a second copy of credentials. The writer never opens
the tools file or serializes its contents.

Writes use a private temporary file in the discovery directory followed by an
atomic replacement. Failures are diagnostic only. Readers must validate the
version/app, require a loopback address, check liveness, and tolerate stale
files after a crash or shutdown. A file alone is not evidence of a live engine.

Sister apps must ask before reading the referenced Composio provider. No job
submission capability is advertised: arbitrary calls must not be inferred
from the existence of the RPC address. Existing conversation links remain
`zeron://open/chat/<id>?workspace=<locator>`.

The additive `zeron://open/meridian?file=<percent-encoded absolute path>` handler
opens an unsent composer draft. It accepts only UTF-8 Markdown files up to 64 KiB
inside an `exports` directory under the canonical macOS Meridian data root.
Symlinks escaping that root are rejected. Existing new-chat draft text is
preserved. Imported text is never submitted automatically. Custom Meridian
`--data-dir` roots are not accepted by this handler.
