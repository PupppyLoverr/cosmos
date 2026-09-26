# zgpui — typed ACP client for Cosmos GPUI

zcli does not link GPUI. Cosmos desktop attaches like this:

1. Spawn `zcli acp` (stdio JSON-RPC).
2. Call `initialize`, then `session/new`, then `session/prompt`.
3. Read `session/update` notifications as `GpuiEvent`.

```rust
use zgpui::client::AcpClient;
let mut c = AcpClient::spawn("zcli".into())?;
c.initialize()?;
let sid = c.session_new(".")?;
let evs = c.prompt(&sid, "ping")?;
```

Permission requests, usage, compact, and self_improve events map to
`GpuiEvent` variants. No local HTTP server. No GPUI types in this crate.
