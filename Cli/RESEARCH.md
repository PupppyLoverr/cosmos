# RESEARCH.md

Research for the Cosmos CLI family. All clones live in `.research\` (gitignored), each
`git clone --depth 1`. Retrieval date for all sources: 2026-09-22.

## grok-build — `github.com/xai-org/grok-build`

- SHA `4247f661689354b831191f11eeeac8424993fe3d`, license Apache-2.0 (`LICENSE`).
- Real xAI Grok agent. ~90 crates under `crates/codegen/xai-*`.
- Read: `crates/codegen/xai-grok-config-types/src/lib.rs` (RemoteSettings + config merge),
  `xai-grok-config-types/src/registry.rs` (feature registry: env > config > remote > default),
  `xai-grok-hooks/src/event.rs` (`HookEventName`: `session_start`, `user_prompt_submit`,
  `pre_tool_use`, `post_tool_use`, `stop`, … with alias-tolerant parsing),
  `xai-grok-tools/src/util/hash.rs` (FNV-1a line fingerprints for `hashline` edits),
  `xai-grok-tools/src/implementations/grok_build_hashline/` (`anchor.rs`, `edit/apply.rs`,
  `read_file.rs` — `read` emits `LINE#HASH` anchors, `edit` anchors on them),
  `xai-acp-lib/` (thin wrapper over the official `agent-client-protocol` crate, `unstable` feature),
  `xai-grok-session-events`, `xai-fast-worktree`, `xai-grok-subagent-resolution`.
- Take: hash-anchored read/edit; hook events as a closed enum with aliases; feature registry
  resolving env > config > remote > default; worktree-per-subagent via git.
- Reject: per-crate sprawl (90 crates) and remote-settings/control-plane plumbing — we keep
  seven crates and a static contract. Their hash uses whitespace-normalized FNV-1a; we use raw
  SHA-256 prefix per contract (simpler, still stable).

## codex — `github.com/openai/codex` (`codex-rs/`)

- SHA `94174e44cbc54cece45f6052328ca0c2cd7a8a2a`, license Apache-2.0.
- Read: `codex-rs/protocol/src/protocol.rs` (`Op` submission enum, `EventMsg` event enum,
  `AskForApproval { UnlessTrusted, OnRequest, Granular, Never }`,
  `SandboxPolicy { DangerFullAccess, ReadOnly, ExternalSandbox, WorkspaceWrite }`,
  `NetworkAccess { Restricted, Enabled }`),
  `codex-rs/exec/src/` (`codex exec` headless mode with `event_processor_with_jsonl_output.rs`),
  `codex-rs/windows-sandbox-rs/src/` (dedicated sandbox user + ACL grants + WFP firewall +
  restricted token — heavyweight, ~60 files),
  `codex-rs/linux-sandbox`, `codex-rs/bwrap` (bubblewrap),
  `codex-rs/core/src/compact*.rs` (remote + local compaction variants),
  `codex-rs/execpolicy/` (exec policy rules engine).
- Take: Op/Event split as one protocol crate; `AskForApproval` enum shape;
  `--output json` headless event stream; sandbox-as-library with explicit degraded fallback.
- Reject: the Windows sandbox user + WFP firewall approach — it creates machine-local users
  and firewall state; our contract uses AppContainer + Job object instead (no system mutation).
  Also reject their multi-tier approval granularity; we keep allow/ask/deny.

## pi — `github.com/earendil-works/pi`

- SHA `95fbc04997eaee961eb673fa7923e9220609ebd5`, license MIT (`LICENSE`). TypeScript monorepo.
- Read: `packages/agent/src/agent-loop.ts` (`agentLoop`/`agentLoopContinue` emit `AgentEvent`
  on an `EventStream`; context holds `AgentMessage[]`, converts to provider `Message[]` only at
  the LLM boundary), `packages/coding-agent/src/core/agent-session.ts`,
  `core/session-manager.ts`, `core/session-export.ts`,
  `core/compaction/branch-summarization.ts` (branch summarization when forking sessions).
- Take: keep one internal message type and translate at the provider boundary (our `zproto`
  does exactly this); session export as a first-class command.
- Reject: event-stream machinery is clever but tied to TS async iterables; Rust uses a plain
  append-only JSONL writer.

## oh-my-pi — `github.com/can1357/oh-my-pi`

- SHA `df624f56b0508c51067a70422606cac898ac2bcb`, license MIT. Bun/TS + Rust natives.
- Read: `crates/pi-edit/src/modes/hashline/` (`parser.rs`, `patcher.rs`, `mismatch.rs`,
  `recovery.rs`, `block.rs` — full hashline edit mode with stale-anchor rejection and
  mismatch reporting), `crates/pi-natives/src/grep.rs`, `ast.rs` (native grep + AST tools),
  `packages/catalog` (KDL rule tree for model policy — noted, not ported).
- Take: hash-anchored edit is proven at scale; their `mismatch.rs` returns fresh context around
  failed anchors — same idea as our `STALE` + fresh anchors ±5 lines.
- Reject: multi-mode edit engine (sloppy/patch/hashline) — we ship one mode only.

## opencode — `github.com/anomalyco/opencode` (mirror of `sst/opencode`, same HEAD)

- SHA `fe3f3a41f79ad292cc3c7c629567385a20ec5130`, license MIT.
- Read: `packages/opencode/src/permission/` (allow/ask/deny evaluation),
  `src/provider/provider.ts` + `packages/core/models-dev` (models.dev as the provider/model
  catalog source), `src/server/` + `src/cli/` (client/server split over HTTP),
  `packages/web/src/content/docs/permissions.mdx` (config schema: `"permission": {tool: "allow"|"ask"|"deny"}`).
- Take: models.dev as the price/context catalog source (used for `config.example.toml` prices);
  allow/ask/deny as the permission vocabulary — matches our `[permissions]` table.
- Reject: client/server-over-HTTP split — our harnesses are single-process; sessions are files.

## kimi-cli — `github.com/MoonshotAI/kimi-cli`

- SHA `5c7db06c24a175b17a3fb44fa58872235c1888e8`, license Apache-2.0. Python 3.12+, uv workspace.
- Read: `src/kimi_cli/soul/compaction.py` (`should_auto_compact`: fires when
  `tokens >= max * trigger_ratio` OR `tokens + reserved >= max`; `SimpleCompaction` summarizes
  old messages, preserves last N user/assistant turns),
  `src/kimi_cli/soul/kimisoul.py` (core loop), `src/kimi_cli/wire/types.py` (Wire protocol:
  `TurnBegin`, `SteerInput`, `TurnEnd`, approval events — event-sourced UI feed),
  `src/kimi_cli/session*.py` (session dirs + fork), `packages/kosong` (provider abstraction),
  `packages/kaos` (OS abstraction: local vs SSH file/exec).
- Take: compaction trigger = ratio OR reserved-headroom (we extend with cache-cold rule);
  preserve-last-N-messages on compaction; separate `kosong` provider layer.
- Reject: their session state mixes checkpointed context objects with wire logs; we keep one
  append-only JSONL as both transcript and resume source.

## devin-cli — `github.com/CognitionAI/devin-cli` + `docs.devin.ai/cli`

- SHA `bd4163ed29e934b898f185752182c91aacac20f7`. Repo is a stub: `README.md` + a release
  script only; no source, no LICENSE file. Real research via docs (retrieved 2026-09-22):
  `docs.devin.ai/cli` (local agent; `devin -p`, `--resume`),
  `docs.devin.ai/cli/cloud` (`devin --cloud` streams a VM session; `/handoff`/`/pickup` move
  work local↔cloud; `--cloud -r` resumes by URL or id; `/model`, `/repo`, `/platform`).
- Take: handoff as a first-class verb (`/handoff` moves local→cloud; `/pickup` cloud→local) —
  our `zdev handoff`/`resume --handoff` mirrors this as a file format, not a service.
- Reject: knowledge/playbooks are cloud-side per docs ("not yet supported in CLI"); we keep
  playbooks local in the Meridian vault instead.

## antigravity-cli — `github.com/google-antigravity/antigravity-cli`

- SHA `ad7d70342a108687a1b36db7573050de3e9c2c3e`. Repo is a stub: README + `examples/`
  (statusline, title scripts, images); no source, no LICENSE file. Docs live at
  `antigravity.google/docs/cli` (linked from README).
- Read: `README.md` (terminal-first sibling of the Antigravity IDE; "optimized for … remote SSH
  sessions with minimal resource overhead"; shared agent engine; session export to the GUI),
  `examples/statusline/` + `examples/title/` (shell hooks writing terminal statusline/title).
- Take: SSH-first posture is a stated design goal — validates zagy's remote detection and
  no-GUI-prompt rules; statusline-via-shell-script is their extension surface (we use hooks).
- Reject: nothing else available — no `localharness` source exists in the public repo; the
  brief's claim about it is unverifiable. Documented as BLOCKED-by-absence.

## ZCode (Z.ai)

- No public source repo; research via public docs + privacy reports (retrieved 2026-09-22).
- `docs.z.ai` describes a task-centric coding CLI; community privacy analyses report the
  client uploads repo context (file contents, git metadata) to Z.ai servers beyond the model
  request path, plus analytics.
- Take: nothing — used as the negative reference.
- Reject: any repo-content upload outside provider calls. Enforced by contract §3.12: the
  outbound host allowlist is the provider `base_url`s + configured MCP endpoints only.

## Standards

- **MCP** (`modelcontextprotocol.io`, retrieved 2026-09-22): JSON-RPC 2.0 over stdio;
  `initialize` → `notifications/initialized` → `tools/list`, `tools/call`. Server spawns as a
  child process; env in config.
- **ACP** (`agentclientprotocol.com`, retrieved 2026-09-22): JSON-RPC over stdio;
  agent-side methods `initialize`, `session/new`, `session/prompt`, `session/cancel`;
  agent→client notifications `session/update`; client-side `session/request_permission`.
  Official Rust crate `agent-client-protocol` exists (grok-build uses it, `unstable` feature).
  No official Go/Python binding verified — zagy/zdev implement the JSON-RPC methods directly.
- **SKILL.md frontmatter**: `---\nname: …\ndescription: …\n---` YAML; body is markdown the
  model reads on demand. Matches `.agents/skills/*/SKILL.md` discovery in this repo.
- **Claude Code hooks** (public docs): events `SessionStart`, `UserPromptSubmit`,
  `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `SessionEnd`, `PreCompact`; JSON on
  stdin; exit 0 = ok, exit 2 = block with stderr fed back to the model. Grok's
  `HookEventName` (above) mirrors the same vocabulary — our event names align.

## Provider APIs

- **OpenAI Chat Completions** (`platform.openai.com/docs/api-reference/chat/create`,
  retrieved 2026-09-22): `usage.prompt_tokens_details.cached_tokens` for cache reads;
  `prompt_cache_key` request field routes to the same cache bucket; newer
  `prompt_cache_breakpoint {mode:"explicit"}` marks a boundary (verified in spec, we send
  `prompt_cache_key` only — most OpenAI-compatible endpoints accept or ignore it).
- **Anthropic Messages** (`platform.claude.com/docs/en/build-with-claude/prompt-caching`,
  retrieved 2026-09-22): `cache_control: {"type":"ephemeral"}` on content blocks (or top-level
  automatic); response `usage.cache_read_input_tokens` / `cache_creation_input_tokens`;
  5-minute default TTL, refreshed on each hit (measured from request start); 1-hour TTL exists
  at higher write cost. Cache write = 1.25× input; read = 0.1× input.
- **Gemini `generateContent`** (`ai.google.dev`, retrieved 2026-09-22): response
  `usageMetadata.cachedContentTokenCount`; implicit caching on 2.5+ models; explicit
  `cachedContent` resource for manual caches. Cache read price ~0.1× input.
- **xAI** (`docs.x.ai/docs/models`, retrieved 2026-09-22): OpenAI-compatible surface at
  `https://api.x.ai/v1`; cached input billed separately (e.g. grok-4.7 $2.00 in / $0.50 cached /
  $6.00 out per 1M); grok-build-0.1 $1.00/$0.20/$2.00, 256k ctx.
- **Prices used in `config.example.toml`** (source: `models.dev/api.json`, retrieved
  2026-09-22; USD per 1M tokens):
  - `gpt-5.6`: in 4.00, cache_read 0.40, cache_write 5.00, out 20.00, ctx 1050000
  - `claude-sonnet-4-6`: in 3.00, cache_read 0.30, cache_write 3.75, out 15.00, ctx 1000000
  - `gemini-2.5-pro`: in 1.25, cache_read 0.125, cache_write 0, out 10.00, ctx 1048576
  - `grok-4.7`: in 2.00, cache_read 0.50, cache_write 0, out 6.00, ctx 500000
- Meridian app vault inspected at `C:\Users\ravth\Meridian` (read-only): SQLite `vault.db` +
  Loro docs under `%APPDATA%\Meridian\<profile>\` (`crates/vault/src/lib.rs`); schema is
  tasks/events/pages/meetings/briefings — the desktop app's own store, not an agent memory
  vault. Bridge uses `MERIDIAN_VAULT` (default `%USERPROFILE%\.meridian\vault`) per contract
  and never writes to the app vault.
