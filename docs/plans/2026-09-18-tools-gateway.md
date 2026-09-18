# cosmos-tools plan — Composio Connect + user MCP servers for every harness

Goal: any agent harness Cosmos drives (claude, codex, cursor, devin, grok,
hermes, pi, opencode, amp, antigravity, mock) can call real-world tools —
Gmail, Notion, Slack, and the rest of Composio's catalog — plus any MCP
server the user configures, from one Settings page, with no
harness-specific setup.

Constraints locked with the user (2026-09-18):

1. The injected server is named **`cosmos-tools`** — matching
   `cosmos-browser` / `cosmos-computer`.
2. Composio integration is **Composio Connect with the consumer API key**:
   `https://connect.composio.dev/mcp` + `x-consumer-api-key: ck_…`
   (dashboard → Sessions & API Key). OAuth is the endpoint's other auth
   mode and stays a later option on the same provider entry.
3. Tool scope is **per-device global**, same boundary as agent accounts;
   per-chat/per-space selection is deferred.
4. **Binary budget: ≤ ~2–5 MB added.** Everything below is in-tree Rust on
   deps already in the workspace (tokio, reqwest, serde_json, tokio-util,
   url, base64) — no new external dependencies, no bundled runtimes, no
   downloaded components. Expected delta is well under 1 MB compiled.

## The precedent being generalized

Cosmos already stamps loopback MCP endpoints onto every harness, and there
are now two working instances of the pattern:

| Harness | Channel | Site |
| --- | --- | --- |
| claude | `--mcp-config` JSON http entry + env | `crates/harness/src/claude/mod.rs` |
| amp | `--mcp-config` JSON + env | `crates/harness/src/amp.rs` |
| codex | `config.mcp_servers.<name>.url` on `thread/start` + env | `crates/harness/src/codex/mod.rs` |
| ACP (devin, grok, hermes, pi) | `session/new mcpServers` → stdio shim (`cosmos browser-bridge`, `cosmos computer-stdio`) | `crates/harness/src/acp/mod.rs` |
| cursor / antigravity / opencode | `COSMOS_*_MCP` env + prompt JSON-RPC recipe | env stamp only |

Existing machinery: `BROWSER_BRIDGE_OPTION` / `COMPUTER_OPTION` in
`RunRequest.model_options` (`crates/proto/src/agent.rs`), `prepare_*_prompt`
guidance blocks, `*_acp_server` shim entries, `*_env` fallbacks
(`crates/harness/src/lib.rs`), the loopback MCP-subset HTTP server
(`crates/ui/src/browser/bridge.rs`), and — landed in-flight by the
computer-use work — **`crates/computer`**: an engine-owned crate containing
the loopback endpoint (`endpoint.rs`), a stdio MCP client
(`mcp_client.rs`), the `computer-stdio` front (`stdio.rs`), a device-local
settings service (`service.rs`), and an optional downloaded component
(`install.rs`).

`cosmos-tools` is the third instance, shaped exactly like `zeron-computer`.

## Why a gateway, not pass-through stamping

- **Secrets stay off argv.** Composio's `x-consumer-api-key` and user MCP
  headers would otherwise land in `--mcp-config` JSON on process command
  lines. The gateway holds them; children get
  `http://127.0.0.1:<port>/mcp/tools/<token>` only.
- **One URL for every harness**, including the env-fallback ones that
  cannot consume headers at all.
- **Aggregation point**: Composio + arbitrary stdio/HTTP MCP servers merge
  into one namespaced `tools/list`.
- Engine-owned ⇒ works headless and on remote host devices; the agent
  always reaches its own engine's loopback.

## Composio Connect — what the endpoint actually is

Verified against docs.composio.dev (2026-09-18) and the dashboard
screenshot:

- Streamable-HTTP MCP at `https://connect.composio.dev/mcp`; auth is
  `x-consumer-api-key: ck_…` header **or** OAuth — we use the header.
- It exposes **7 meta-tools**, not per-app tools:
  `COMPOSIO_SEARCH_TOOLS` (catalog search + execution plan),
  `COMPOSIO_GET_TOOL_SCHEMAS`, `COMPOSIO_MULTI_EXECUTE_TOOL` (≤50 parallel),
  `COMPOSIO_MANAGE_CONNECTIONS` (create/list/remove app OAuth links),
  `COMPOSIO_WAIT_FOR_CONNECTIONS`, `COMPOSIO_REMOTE_WORKBENCH`,
  `COMPOSIO_REMOTE_BASH_TOOL`.
- **Per-app auth is meta-tool-driven**: the agent (or our settings page)
  calls `COMPOSIO_MANAGE_CONNECTIONS`, gets back an OAuth link, the user
  approves it in a browser, and the connection persists across sessions.
  Links are short-lived; retry regenerates.
- Key consequence: **no Composio REST API is needed at all.** App-connect
  buttons and connected-app status in Settings ride the same two
  `tools/call`s (manage/list) through our own gateway client.
- **Composio CLI — evaluated and rejected**: `curl … | sh` + `composio
  login` requires an external install and mutates per-harness config files
  — the opposite of uniform injection, and it can't reach env-fallback
  harnesses. Direct HTTP does the same job with zero installs. (A user who
  wants it anyway can add it as a custom stdio server.)

## Architecture

```
harness (any) ── stamped mcp config ──► cosmos-tools @ 127.0.0.1/mcp/tools/<token>
                                          │  tools/list = fan-out + prefix
                                          │  tools/call = strip prefix → route
                              ┌───────────┼───────────────┐
                       Composio Connect   stdio MCP     remote HTTP MCP
                       (x-consumer-api-   server child  (+ headers)
                        key, meta-tools    spawned on
                        drive app OAuth)   host engine
```

Tool names on the gateway wire are `<provider>__<tool>` — e.g.
`composio__COMPOSIO_SEARCH_TOOLS`, surfaced to agents as
`mcp__cosmos-tools__composio__COMPOSIO_SEARCH_TOOLS` in claude-style
naming. Deterministic prefixing, collision-free across providers.

## Workstreams

### T0 — `crates/tools` (`zeron-tools`), mirroring `zeron-computer`

```
crates/tools/src/
  lib.rs       — ToolError (retryable codes like ComputerError), provider
                 types, ToolStore (load/save {data_dir}/tools.json),
                 POPULAR_APPS + PRESET_SERVERS consts
  endpoint.rs  — loopback MCP server; adapted copy of computer's
                 endpoint.rs (127.0.0.1 + 128-bit path token, same JSON-RPC
                 subset, ≥90s CALL_TIMEOUT for COMPOSIO_MULTI_EXECUTE)
  service.rs   — ToolService: provider store, upstream lifecycle, fan-out
                 tools/list (prefixed merge), route tools/call, app-connect
                 flows, watch channel for the settings page
  upstream.rs  — HttpUpstream (reqwest POST + static headers) and
                 StdioUpstream (NDJSON child; adapt computer's
                 mcp_client.rs), lazy connect + reconnect, tools/list cache
  composio.rs  — Connect endpoint const, api-key header, thin helpers over
                 the meta-tools (connect/list/search apps)
  stdio.rs     — `cosmos tools-stdio --url` front (same as computer's)
  mock.rs      — fake upstream for tests
  tests.rs
```

Cargo.toml matches `zeron-computer`'s dep shape — all workspace deps
already in the tree. A later cleanup can extract the shared
endpoint/client plumbing into a `zeron-mcp` crate consumed by browser,
computer, and tools — deferred deliberately to stay out of the in-flight
computer work's way.

### T1 — stamping + injection (proto + harness + engine)

- `crates/proto/src/agent.rs`: `TOOLS_OPTION = "cosmos_tools"`,
  `tools_url(request)`, add to `is_internal_option`.
- `crates/harness/src/lib.rs`: `TOOLS_ENV = "COSMOS_TOOLS_MCP"`,
  `prepare_tools_prompt` (`<cosmos_tools>` block naming providers and the
  env-var recipe), `tools_acp_server` (`cosmos tools-stdio --url …`),
  `tools_env` — the same four lines of shape as the computer helpers.
- Injection sites: add `cosmos-tools` beside `cosmos-browser` /
  `cosmos-computer` in claude's and amp's `--mcp-config` maps, codex's
  `mcp_servers.cosmos-tools.url` config override, ACP's `session/new
  mcpServers` chain, and `tools_env` in cursor/antigravity/opencode.
- `crates/engine`: `ToolService::new(data_dir)` assembled in `EngineCore`
  beside `sessions`; endpoint serve task; `dispatch_inner` stamps
  `model_options["cosmos_tools"] = {"url": endpoint_url}` when ≥1 provider
  is enabled — engine-owned, so no lease/renewal (same as computer, unlike
  the UI-window-scoped browser bridge).
- `apps/zeron/src/main.rs`: `ToolsStdio { url }` subcommand dispatched
  before tracing init. (The computer work needs the same wiring for
  `computer-stdio` — flag it; it's three lines.)
- `<cosmos_tools>` guidance stays short: server name, the
  SEARCH→SCHEMAS→EXECUTE recipe for Composio, "connect links may appear —
  surface them for the user", env-var fallback. Stamped only when ≥1
  provider is enabled; title/discovery runs already drop `model_options`.

### T2 — RPC surface (forwardable, matching agent accounts)

`ListTools` (providers, masked config, status, per-provider tool counts),
`UpsertToolProvider`, `RemoveToolProvider`, `SetToolProviderEnabled`,
`ConnectComposio` (store + validate key via a real `tools/list`),
`DisconnectComposio`, `ListComposioApps` (manage-connections list →
per-app status), `StartAppConnect { toolkit }` → `{auth_url}` (calls
`COMPOSIO_MANAGE_CONNECTIONS` through the upstream client),
`PollAppConnect`, `ProbeMcpServer` (test config → tool count/sample
names), `WatchTools` stream. All added to `forwardable()` so the page's
device switcher configures tools where the agent runs.

### T3 — Settings → Tools page

New `SettingsSection::Tools` ("Tools") + `crates/ui/src/settings/tools.rs`,
modeled on `accounts.rs` (provider cards, status chips, inline failure
states, loading skeletons, page-header device switcher with
`targetDeviceId` passthrough). Three groups:

1. **Apps (Composio)** — card: `ck_…` key field → Connect → validated
   connected state (key masked, Copy/Remove). Below it the **popular apps
   grid**: ~10 everyday picks (`POPULAR_APPS` const: gmail,
   google_calendar, google_drive, slack, notion, github, linear, outlook,
   google_sheets, todoist — toolkit slugs + icons), each tile with
   Connect / Connected chip / Reconnect. `StartAppConnect` → open the
   returned auth URL (embedded browser honoring `open_web_links_in_zeron`,
   else system) → `PollAppConnect` until connected. Plus a search field →
   `COMPOSIO_SEARCH_TOOLS` passthrough → toolkit rows → same flow, so all
   1000+ apps are reachable.
2. **MCP servers & plugins** — rows per configured server: name, transport
   badge (stdio/http), status + tool count, enable toggle, remove.
   Add-server form: name + kind picker + (command/args/env |
   url/headers). `PRESET_SERVERS` suggestion chips for one-click adds
   (Figma local dev-mode MCP, Pencil, Playwright, GitHub, Notion, Linear…)
   that prefill the form. Presets needing OAuth are marked "sign-in
   required" — remote-MCP OAuth is a listed follow-up; header-auth and
   local servers work today.
3. **Computer use** — the section bound to `zeron-computer`'s service:
   enable toggle, `ControlMode` picker (observe-only / background / full
   control), driver install + progress + OS-permission status. The other
   agent owns the crate; this page is its settings surface (their error
   text already points users at "Settings → Computer" — coordinate the
   section placement with that work; a standalone section is the
   fallback).

### T4 — wire-through verification

- Unit tests in `zeron-tools`: prefix routing, fan-out merge, upstream
  failure isolation, tools/list caching, secret hygiene (assert no secret
  reaches `model_options`/argv beyond the loopback URL), `RuntimeConfig`
  stability.
- Harness fixture tests asserting `cosmos-tools` appears in each driver's
  stamped config (`crates/harness/tests/fixtures/`).
- RPC tests: forwardable registration, masked replies.
- Live: headed run against real Composio Connect — key connect →
  `tools/list` shows 7 meta-tools → gmail connect link → fetch mail from a
  claude run and a codex run.
- Sweep: `cargo fmt --all -- --check`, `cargo check`/`test` on touched
  crates.

## Deferred

- OAuth auth mode on the Composio provider (and remote-MCP OAuth
  generally — the presets marked "sign-in required" unlock with it).
- Per-chat/per-space tool scoping via chat config.
- `zeron-mcp` shared-plumbing crate (dedup of browser/computer/tools
  endpoint+client code).
- Composio API-key `tool_router/session` provider kind (org-managed mode).
- Real plugin bundles (MCP + skills + prompts) — preset chips cover the
  UX today.
