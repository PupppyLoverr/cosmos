# EVIDENCE.md

- Date: 2026-09-22
- Machine: Windows 11 (windows/amd64), PowerShell 5.1 + Git Bash
- Repo root: `C:\Users\ravth\zeron` (Cosmos checkout), HEAD `bd919efe`

## Preflight

```
rustc 1.97.1 (8bab26f4f 2026-07-14)
cargo 1.97.1 (c980f4866 2026-06-30)
go version go1.26.5 windows/amd64
uv 0.12.9 (9f9286029 2026-09-01 x86_64-pc-windows-msvc)
python --version -> not installed (Microsoft Store stub); uv manages Python for python-cloud
git version 2.54.0.windows.1
ripgrep 15.0.0 (rev 3a612f88b8) +pcre2 +jit
ollama --version -> command not found; http://localhost:11434/v1/models -> timed out
Get-Command zcli,zdev,zagy,cosmos-cli -> no results (no name collision; binary name is `zcli`)
Test-Path $env:USERPROFILE\.cosmos-cli -> False
```

### Live endpoint

`COSMOS_EVAL_BASE_URL`, `COSMOS_EVAL_MODEL`, `COSMOS_EVAL_API_KEY_ENV` are all empty.
No Ollama installed or listening on `localhost:11434`.
No `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `XAI_API_KEY`, `GEMINI_API_KEY` in the environment.

**Result: NO LIVE ENDPOINT.** Provider paths are proven with the deterministic
`fixture` provider per contract; the eval table below uses fixture runs.

### Pre-run git snapshot

`Cli\` existed but was empty at start; all content under `Cli\` was created in
this session.

### Meridian vault inspection (read-only)

`C:\Users\ravth\Meridian\crates\vault\src\lib.rs` defines an application vault:
SQLite `vault.db` plus `loro/` document snapshots (tasks, events, pages,
meetings, briefings) — a productivity domain, not an agent memory vault.

Decision: the bridge implements the markdown vault from the contract at
`MERIDIAN_VAULT` (default `%USERPROFILE%\.meridian\vault`) and never writes to
the Meridian app vault.

## Research SHAs (`.research\`, depth-1 clones)

```
antigravity-cli  ad7d703   codex        94174e4   devin-cli   bd4163e
grok-build       4247f66   kimi-cli     5c7db06   oh-my-pi    df624f5
opencode-sst     fe3f3a4   opencode     fe3f3a4   pi          95fbc04
```

## Acceptance command log

### meridian-bridge

`cargo test` — 15 passed (vault, BM25 search, secret scanner, deterministic
summarizer, o200k tokenizer w/ vendored ranks, JSON-RPC server, CLI).
Python client: `uv run --with pytest pytest` — 3 passed.
Go client: `go test ./...` — ok.
`meridian boot` on the real zeron repo emits the embedded CONTRACT.md system
snippet + skill index + repo map within the boot budget (533 tokens observed).

### zsandbox (Windows)

`cargo test -p zsandbox -- --test-threads=1` — 4 passed:

1. write inside workspace succeeds under AppContainer token (S-1-16-4096 low IL,
   deny-only groups, package SID present);
2. write outside workspace denied (`C:\Windows\Temp` write fails);
3. network denied by default (`curl` → "Could not resolve host") and allowed
   with `--net allow` (internetClient capability, SE_GROUP_ENABLED);
4. timeout kills runaway process (infinite `for /l` loop → exit 124, no hang).

Degradation chain AppContainer → restricted token → plain spawn inside the Job
reports its reason; AppContainer profile setup is serialized (parallel-test race
fixed) and PATH-dir grants are cached in a state file so repeat execs are fast.

### rust-tui workspace

`cargo test --workspace` — 21 passed, 0 failed
(ztools 9, zproviders 4 incl. §12 allowlist rejection, zcore 4, zsandbox 4).
`cargo fmt --all` / `--check` clean; `cargo clippy --workspace --all-targets` 0 warnings.

`zcli -p "ping" --output json-stream` emits session_start → user → assistant
("pong") → usage → session_end; `session_start.data.boot_tokens` = 327 with
`MERIDIAN_BIN` set (real bridge boot context).

### zdev (python-cloud)

`uv run --with pytest pytest` — 7 passed. `zdev tools dump` byte-identical to
`contracts\tools.json`. Cross-harness resume verified (appended events to a
zcli session; `harness` field flips to `zdev` on new events).

### zagy (go-ssh)

`go test ./...` — 8 passed (read/write/edit, STALE + escape denial, session
roundtrip+branch, event shape, tools.json identity, host allowlist, fixture
ping/usage). `zagy tools dump` byte-identical.

### Conformance — `powershell -File eval\conformance.ps1`

```
PASS  binaries built
PASS  tools dump byte-identical
PASS  zcli fixture ping/json-stream
PASS  zdev fixture ping/json-stream
PASS  zagy fixture ping/json-stream
PASS  session list/export/branch/tree
PASS  cross-harness session resume        (zcli→zdev→zagy, one session file, 3 harnesses)
PASS  memory add/search via bridge        (zcli add, zcli+zagy search hit)
PASS  sandbox probes (4)
PASS  plan-mode deny + path escape deny
PASS  zcli acp handshake                  (initialize→session/new→session/prompt→pong)
PASS  zdev acp handshake
PASS  zagy acp handshake
PASS  task worktree create+cleanup        (cosmos-task-* branch, removed when clean)
PASS  tool call terminates in 1 turn      (fixture tool_result → "done.")
conformance: 15/15 PASS
```

### Eval — `powershell -File eval\run.ps1`

```
PASS  t01-read-edit-write/zcli
PASS  t01-read-edit-write/zdev
PASS  t01-read-edit-write/zagy
eval: 3/3 PASS
```

t01 exercises the real contract flow: `tool:read` returns `file_sha256=` +
`n:hash|` anchors → `tool:write` creates `notes.md` → `tool:edit` consumes the
harvested sha+anchor (`v1`→`v2`) → `tool:grep` finds it → all session files
validate against `eval/event.schema.json`.

## Eval table

| task | zcli | zdev | zagy | notes |
|------|------|------|------|-------|
| t01-read-edit-write | PASS | PASS | PASS | fixture provider, deterministic |

## Notable fixes during the run

- `\\?\` verbatim paths broke cmd's cwd — stripped in `ToolContext::new`.
- AppContainer profile creation raced under parallel tests — serialized.
- `SetNamedSecurityInfoW` stalled on the `C:\nvm4w\nodejs` junction — grants now
  run in bounded worker threads and are cached.
- Fixture provider re-issued tool calls forever in Go — all three fixtures now
  answer `done.` when the last message carries a `tool_result`.
- `memory add` passed an invalid `note` type — all three CLIs default to `map`
  and accept `--type`; the tool itself rejects a missing `type`.
- PowerShell 5.1 eats `"` inside native args — eval scripts use `\"` escapes.

## Live provider pass (2026-09-23)

BYOK verified via 9Router (local OpenAI-compatible gateway, `localhost:20128/v1`,
key in `ROUTER9_API_KEY`). `kr/claude-sonnet-4.5` (Kiro free tier) served real
completions on all three harnesses: `zcli -p ping`, `zagy -p ping`,
`zdev -p ping` returned live assistant text + real usage counts, and a live
`ls` tool call ran the full model→tool→result→summary loop on zcli.
`ag/gemini-3.8-flash` also verified. Fixes this exposed: §12 allowlist kept the
URL port (all three impls), `config.example.toml` model aliases containing `.`
parsed as nested TOML tables (renamed to dashes), zagy TOML parser dropped
trailing comments on quoted strings. `cu/*` (Cursor route) returns
`content: null` for plain chat completions — Cursor's subscription only serves
its own agent API, not OpenAI-style inference.

## TUIs (2026-09-23)

Each harness now has a real interactive UI (headless `-p`, `acp`, and all
subcommands unchanged; non-TTY falls back to the line REPL):

- `zcli` — ratatui+crossterm (`ztui`: tui/render/markdown/wrap/slash/worker),
  Devin-CLI look: pixel banner, highlighted user bar, `●/⎿` tool lines,
  spinner busy line, bordered composer, `/` menu, `/model` picker, permission
  radiogroup, `?` shortcuts overlay. Agent runs on a worker thread
  (`Asker` is now `Send`). 41 workspace tests incl. TestBackend frame tests.
- `zagy` — bubbletea/bubbles/lipgloss (`tui.go`, `tui_render.go`),
  Antigravity look: block logo banner, cyan `>` composer, `▶ Thought for Ns`,
  `/` autocomplete, `?` tabbed shortcuts overlay, permission card, footer
  `? for shortcuts · model · mode`. 14 Go tests incl. a real PTY-free
  tea.Program end-to-end over io.Pipe.
- `zdev` — Ink+React TypeScript app `python-cloud/ink` (bin `zdev-ui`,
  or `uv run zdev ui`) driving the Python backend over ACP; brainless /
  Claude-Code look: welcome header, `❯` user chip, `⏺` tool cards with
  expandable detail, `✶` thinking line, permission radiogroup, slash menu,
  `/model` picker via `session/list_models`. Backend `_acp` gained:
  server→client `session/request_permission`, `session/set_mode`,
  `session/set_model`, `session/approve`, `session/list_models`, `usage`/`plan`
  update kinds, stdin reader thread, per-session cancel. `exec_tool` now emits
  `tool_call` events. bun test 6/6; PTY e2e verified.
- Esc interrupt is cooperative everywhere (checked between turns/tool calls —
  an in-flight HTTP request can't be aborted).

## zdev rebuilt as TypeScript/bun (2026-09-23, later)

`zdev` was rebuilt from scratch as a single compiled binary — the Python
package and the separate Ink front-end were replaced by
`Cli/zdev` (TypeScript + Ink + React, bun). `bun build --compile` produces
`dist/zdev.exe` (~90 MB, no runtime deps); it is installed globally at
`~/bin/zdev.exe` (`zdev-ui` shims kept as compat aliases → `zdev ui`).

- Backend ported: config/TOML, session store, providers (fixture/openai/
  anthropic/google), SSE, tool dispatch, permissions, sandbox, ACP server,
  memory/meridian client, worktrees.
- TUI merged in-process (was: Ink app driving `zdev acp` over stdio — same
  UX, no subprocess). Brainless registry sources installed under
  `src/components/brainless/` and ported to Ink preserving semantics
  (`❯` user chip, `⏺` tool cards, `✶` thinking, radiogroup permission,
  listbox pickers). `pied-piper-onboarding` ported as the first-run flow
  (typed lines, env checks, model picker) with a Milky-Way spiral logo
  (✺ core / ◉ ring / ░▒▓ arms, blue→purple gradient).
- `eval/conformance.ps1` + `eval/run.ps1` now invoke `zdev\dist\zdev.exe`
  directly — **15/15 conformance, 3/3 eval** with the compiled binary,
  tools dump byte-identical, cross-harness resume intact.
- `bun test` 15/15, `tsc --noEmit` clean.
- Live verified: `zdev -p "Say just the word: pong" --model r9` → `pong`
  (kr/claude-sonnet-4.5 via 9Router, in 6010 out 1).
- `python-cloud/` retained as the reference port (untracked dir, no longer
  on PATH or referenced by eval/docs).

TUI deepened (same day): remaining brainless sources installed via
`bunx --bun shadcn@latest add` (claude-diff/tool-call/permission/slash-menu/
header/session + grok working/tool/event/status/plan/turn-end/session-active)
and ported to Ink:

- **Real diffs** — `edit` expands to claude-diff grammar (`@@ n–m @@` hunks,
  numbered rows, tinted +/-) by reconstructing the old side from prior `read`
  results (`n:sha|line` anchors) through a small LCS differ (`src/diff.ts`).
  `write` renders numbered +rows; `read` detail renders as a numbered listing.
- **Permission previews** — the approval box shows the actual bash command or
  the edit/write diff before "Do you want to proceed?".
- **Pending spinner** — in-flight tools pulse a braille glyph (grok-working).
- **Turn-end footer** — `◆ Turn completed in Ns · ↓ T tokens` after each turn
  (grok-turn-end); cancelled turns keep the interrupt note.
- **Streaming caret** — `▌` on the assistant turn while chunks arrive.
- **Context meter** — `ctx used / limit` in the footer from list_models,
  green→yellow→red by fill (grok-status).
- Claude display names (`Update`/`Read`/`Bash`, cyan args), `task` renders as
  the grok-tool `┃ ◆` card, plans render as a numbered plan viewer
  (grok-plan), markdown gains `>` quotes and `---` rules.
- `bun test` 24/24, tsc clean, conformance 15/15, eval 3/3 on the rebuilt exe.

Revamp pass (same day) — monochrome + product-grade onboarding:

- **B&W theme** — every color in theme.ts + component hexes is grayscale;
  diff tints are dark/lifted grays instead of red/green.
- **Galaxy is generated, not art text** (`src/components/Galaxy.tsx`) —
  density-field splat of a two-armed logarithmic spiral + gaussian core,
  seeded deterministic → `●•·` dot glyphs, matching the halftone reference
  image. Used by the header logo and the onboarding reveal; width adapts
  to terminal size.
- **First-run onboarding** (`src/ui/Onboarding.tsx`) — starfield warp
  animation (accelerating particles, ~1.6s, any-key skip) → white bloom +
  soft terminal bell → Milky-Way logo dissolves in → typed "WELCOME TO
  COSMOS" → asks your name (`user_name` in config.toml, drives "Welcome
  back <name>") → BYOK picker → typed tour. Lazy `import()` — the module
  never loads once `~/.cosmos-cli/.zdev-onboarded` exists, and the
  component ref is dropped after finish.
- **BYOK catalog** (`src/byok.ts`) — 17 entries: ChatGPT subscription
  (browser OAuth), OpenAI, Anthropic, Google, xAI, Groq, DeepSeek, Mistral,
  Meta Llama, Xiaomi MiMo, Moonshot, Zhipu GLM, Baidu ERNIE, Together,
  OpenRouter, 9Router, Ollama, fixture. Key paste → persisted as a
  User-scope env var (`reg add HKCU\Environment`), provider+model blocks
  appended to config.toml — keys never land in config (contract §12).
- **ChatGPT subscription** (`src/oauth.ts`) — Codex-style PKCE flow:
  browser → auth.openai.com → loopback :1455 → token exchange →
  `~/.cosmos-cli/openai-auth.json` (0600). `zdev login`/`zdev logout` CLI
  commands + `/login` in the TUI. Provider kind `chatgpt` streams the
  Responses API at chatgpt.com/backend-api/codex with refresh-on-401.
  Untestable without a real ChatGPT account — implemented to Codex spec.
- **Mic dictation** (`src/voice.ts`) — `ctrl+m` runs Windows System.Speech
  dictation in a PowerShell child; recognized text lands in the composer.
  `◉ listening…` indicator; degrades with a note if no recognizer/mic.
- **Competitive extras** — `/resume` session picker (SessionPicker.tsx,
  newest-first list → `session/new {resume}`), `@file` mentions inline a
  file as `<file path="…">` context (≤300 lines, cwd-confined), `/login`.
- `bun test` 28/28, tsc clean, conformance 15/15, eval 3/3, fresh-home
  fixture boot verified.

## Open items

- Anthropic/Google-kind adapters exercised only against the fixture (live pass
  used `kind = "openai"`).
- MCP servers: `tool_search`/`mcp_call` dispatch exists and returns
  "no MCP servers configured" when none are set (contract-correct); no MCP
  server was configured on this machine to exercise further.

## 2026-09-26 — three-harness kernel (compact / repair / world / undo)

Date lock from `COSMOS_CLI_MASTER_PROMPT.md`. Windows 11, same machine.
No live model keys. All numbers below are fixture or unit-test.

### Tests

| Suite | Result |
|---|---|
| `cargo test --workspace` in `Cli/rust-tui` | 118 passed, 4 ignored (live lists / mic) |
| `bun test` in `Cli/zdev` | **74/74** |
| `go test ./...` in `Cli/go-ssh` | ok |
| `cargo test` in `Cli/meridian-bridge` | 15 passed (6 lib + 9 integration) |
| `powershell -File Cli/eval/conformance.ps1` | **19/19 PASS** (was 15) |
| `powershell -File Cli/eval/run.ps1` | **9/9 PASS** (t01 + t02-compact-resume + t03-repair × 3 harnesses) |

New conformance rows: `/compact` writes `world/<id>.md` with the goal;
`tool:Read` emits `tool_repair` and a hashline `read`; `/undo` restores the
pre-write bytes after a resumed read→write; fixture `ping` with `--self-improve`
writes a draft skill.

### Compaction (40-turn synthetic, zdev `compact.ts`, boot 400)

Naive full-replay estimate: **327,526** tokens (40 read results × ~80 fat lines).
After L1 persist + L2 snip + forced L5: **10,220** tokens (**3.1%** of naive,
target ≤55%). `world.md` written. Prefix hash unchanged → cache-warm rule holds.

A one-turn fixture `/compact` can *grow* slightly (t02: 403 → 471) because the
structured world snapshot is larger than a single short user line. That is not
a 40-turn regression; the 55% bar is the long-session number above.

### Binary sizes (2026-09-26)

| Binary | Size | Bar |
|---|---|---|
| `rust-tui/target/release/zcli.exe` (fat LTO, strip) | **17.6 MB** | ~18 MB class |
| `go-ssh/zagy.exe` | **11.4 MB** | < 15 MB |
| `zdev/dist/zdev.exe` (`bun build --compile`) | **86.1 MB** | bun compile floor; idle RSS not re-measured this shot |

`zgpui` is in the workspace and has **zero** GPUI crate dependency; daily
`zcli` does not link it.

### What shipped in all three

- Shared `contracts/tools.json` (byte-identical dumps): bash `wait` /
  `from_offset`, `memory_search.scope`, `context_remaining`, `new_context`.
- Five-layer compact + `COSMOS_CLI_HOME/world/<session>.md`.
- Tool-call repair (`Read`→`read`) + doom-loop + hashline STALE ±5.
- Shell fence + middle-out + log persist + `wait=now|next_write|exit`.
- Checkpoints + `/undo` (not git). Read-state restored on `--resume`.
- Shift+Tab: plan → agent → accept-edits → yolo. `--sandbox` × `--ask`.
- `/skillify`, meridian `refine.*`, `--allow-self-mutate` fence.
- Parallel read-only tools (cap 8). `task.write` defaults `worktree`.
- LSP-lite `verify` after write/edit (best-effort `cargo check` / `tsc` / `go build`).

`python-cloud/` is reference-only (`Cli/python-cloud/README.md`).

### macOS CLI convention pass (2026-09-27)

Toolchains installed: Bun 1.4.2 via `npm install -g bun`; Go 1.27.1 via
`brew install go`. Free disk remained **3.5 GiB** after verification.

- Rust: `cargo build --workspace` passed. `cargo test --workspace` passed for
  all crates except the existing macOS path-separator assertion in
  `ztools::tests::workspace_escape_denied` (`..\evil.txt` is a literal
  filename on macOS); 8 passed, 1 failed in that test binary.
- zdev: `bun run build` passed; `bun test` **78 passed, 0 failed**.
- zagy: `go build`, `go vet`, and `go test ./...` passed.
- All three `--help`, `-p ping --model fixture --output-format stream-json`,
  `--continue` after a run, and `completion zsh | head` smoke checks passed.
- `zcli`, `zdev`, and `zagy` now share output/permission aliases, cwd-aware
  continuation, additional roots, system-prompt overrides, project MCP
  discovery, `/cost`, `/init`, and shell completion entry points.

## 2026-10-01 — `cosmostui`: rename, size, sticky compaction, ACP subscriptions

Branch `cosmos-harness`. Same machine. `zcli` was renamed `cosmostui`
(`cosmos.exe` is the desktop app's launcher); a `zcli.exe` copy still works and
prints a one-line notice. zdev/zagy rows unchanged.

### Gates

```
powershell -File eval\size-gate.ps1
PASS  …\target\release\cosmostui.exe is 5.81 MiB (6091776 bytes) <= 8 MiB   (was 17.6-18.7 MB)

powershell -File eval\startup.ps1
baseline (zsandbox --help) median 12.2 ms
PASS  --version median 17.2 ms (+5 ms over baseline, limit +10)
PASS  fixture ping median 48.1 ms (+35.9 ms over baseline, limit +120)

powershell -File eval\conformance.ps1    →  19/19 PASS
powershell -File eval\run.ps1            →  9/9 PASS (t01, t02-compact-resume, t03-repair × 3)
cargo test --workspace                   →  166 passed, 0 failed, 4 ignored (live lists / mic)
cosmostui -p ping --model fixture --output json-stream
  → session_start, user, assistant, usage, session_end  (5 kinds)
tools dump byte-identical to contracts/tools.json (conformance row 2)
```

### Compaction — `zcore/tests/compact40.rs` (40 turns of fat `read`s)

```
A fat reads:   final 22024 / naive 301744 = 7.3% · cumulative 7.3%
               checkpoints 0 · busts 0 · prefix stable true · resume == live true
B 30k window:  final 5740 / naive 67344 = 8.5% · cumulative 26.0%
               checkpoints 7 · busts 7 (≤ checkpoints) · max request 13990
               prefix stable true · resume == live true
```

All model-view edits are sticky: computed once at a checkpoint, replayed
identically on every later request and on `--resume`. The wire prompt is a
fixed prefix — ≤120-token snippet, 8 default tools (~600 tokens; the rest are
reached through `tool_search`/`mcp_call`), boot ≤ 800 tokens.

### What changed per phase

- **0 correctness**: boot `text` field read (was dropped), L4 only collapses
  same-path superseded reads, resume replays the compaction boundary,
  `submit_plan`/`tool_search`/`mcp_call` exposure gated, Anthropic usage merges
  (not sums) `message_start`+`message_delta`, Gemini takes last cumulative
  `usageMetadata`, provider errors exit 4.
- **1 rename+size**: package/bin `cosmostui`; tree-sitter behind `highlight`
  feature (built-in highlighter default), cpal behind `voice`, ratatui-core,
  chrono/ulid dropped for small RFC3339/ULID fns. `eval/size-gate.ps1` and
  `eval/startup.ps1` added.
- **2 prompt+memory**: `ztools::memory` (BM25 bullets, Meridian format, secret
  scan, `COSMOS_MEMORY_DIR`/`MERIDIAN_VAULT`/`COSMOS_CLI_HOME/memory`), no
  meridian subprocess or tokenizer file; Supermemory only when
  `SUPERMEMORY_API_KEY` is set; sticky L1-L5 compaction as above.
- **3 subscriptions**: provider kind `acp` spawns the vendor's own CLI —
  `devin acp`, `agent acp` (Cursor), `codex-acp`; `cosmostui auth login
  devin|cursor|codex` runs the vendor login; `session/request_permission`
  surfaces as the TUI permission prompt; `devin.rs` behind non-default
  `devin-direct` feature; one ureq agent + host allowlist for all HTTP.
- **4 shell**: `wait=now|next_write` spawn through zsandbox, logs keyed by
  command so `from_offset` works, 24/24 middle-out with exact `omitted_chars`,
  137/143 named, real `sandbox:"degraded", reason` in `session_start` and every
  result. Windows fix: PowerShell commands returned empty through the old
  restricted-token path.
- **5 self-improve**: after a clean interactive session, detached
  `cosmostui review --session <id>` writes ≤5 memory bullets + 1 bounded skill;
  `/compact`, `/undo`, `/skillify` run locally (were forwarded to the model);
  write/edit refuse cosmostui's binary, CONTRACT.md and sandbox policy paths
  without `--allow-self-mutate`.
- **6**: §8 ledger in the status line (`turn · in · cache_r · cache_w · out ·
  $ · Σ · ctx%`), `session tree` + `/tree` (branch-aware, short ids usable as
  `--resume <session>@<id>`), README + this entry.

### Decisions / notes

- ChatGPT tokens stay in the shared `credentials.json` (now 0600 on Unix) —
  a second copy would sign the other harness out after one refresh.
- This machine runs `bash` at JobOnly: PowerShell 5.1 can't start its file
  provider inside AppContainer, and the profile's ACLs keep the restricted
  token out. `session_start` and every bash result say so;
  `COSMOS_SANDBOX_TIER=appcontainer|restricted|job` pins a tier.
- ACP plugin turns run the vendor's own agent loop and tools; cosmostui's
  compaction/tools don't apply to those turns.
- `startup.ps1` measures against a baseline exe because Windows process
  creation alone costs ~12-34 ms here.

## 2026-10-01 (later) — ACP live model lists (Devin `configOptions`, Cursor `models`)

`/model` and `cosmostui models` now show the plan's real, current model list
for subscription agents. `live::fetch` no longer refuses `kind=acp`: it spawns
the installed vendor CLI, runs `initialize` + `session/new`, and reads the
model list the agent itself reports — standard `models.availableModels`
(Cursor) or the `configOptions` `category:"model"` select (Devin's dialect).
`session/set_model` is tried first; agents that only advertise a model config
select get `session/set_config_option` instead. Results cache to
`live_models.json` so the TUI picker is instant; `<id>/default` stays on top
and means "the plan decides". `cosmostui auth login <agent>`, `/connect`,
onboarding and `cosmostui models <agent>` all refresh it; bare
`cosmostui models` fills missing caches, in parallel.

Cursor's bins now prefer `cursor-agent` over `agent` — a bare `agent` on PATH
can be another vendor's CLI (this machine resolves `agent` to grok's), which
would have spawned the wrong program and shadowed `installed()`.

### Live evidence (this machine, both CLIs signed in)

```
$ cosmostui models devin     # devin acp → configOptions category "model"
  devin/default                    200k ctx  free / included
  devin/adaptive                   128k ctx  free / included
  devin/swe-2-max                  128k ctx  free / included
  devin/claude-opus-5-5-max        128k ctx  free / included
  … 149 models, vendor order

$ cosmostui models cursor    # cursor-agent acp → models.availableModels
  cursor/default                   200k ctx  free / included
  cursor/default[]                 128k ctx  free / included   (Auto)
  cursor/composer-2.5[fast=true]   128k ctx  free / included
  cursor/claude-opus-5-5[context=300k,effort=medium,fast=false] …
  … 43 models, vendor order
```

`session/set_config_option` verified on real `devin acp`: dispatches (rejects
a bogus session id with "Resource not found", not -32601). Cursor's cold
`session/new` takes ~30 s while its MCP servers connect — the list query
allows 45 s and caches after.

### Gates

```
cargo test --workspace          169 passed, 0 failed (acp_plugin end-to-end now
                                covers list_models→fetch→store→available_models)
size-gate                       PASS  5.83 MiB <= 8 MiB
startup.ps1                     PASS  --version 16.5 ms (+0.2 over baseline),
                                fixture ping 48.6 ms
conformance.ps1                 19/19 PASS
run.ps1 (eval)                  9/9 PASS
```

## 2026-10-02 — cosmostui skins: a look you can change, and a TUI that changes itself

Branch `cosmostui-skins`, seven commits (one per phase). Same machine. Nothing
was removed: the `classic` skin renders byte-for-byte what cosmostui rendered
before. `tests/golden/classic.txt` holds 85 styled lines recorded from the
pre-skins code (banner, info, user turn, markdown, tool ok/error/interrupted,
diff, shell, live region, `/` menu, model picker), and
`view::tests::classic_matches_golden` checks every build against them.

### Gates

```
cargo test --workspace          227 passed, 0 failed, 6 ignored — six runs in a
                                row (two timing-based tests were made
                                deterministic first: the buddy's blink and a
                                wall-clock bound on fence highlighting)
size-gate (new -BaseMiB/-MaxGrowthMiB)
                                PASS  6.48 MiB (6795776 bytes) <= 8 MiB,
                                +0.77 MiB over 5.71 (<= 1)
conformance.ps1                 19/19 PASS
run.ps1 (eval)                  9/9 PASS
clippy -p ztui                  only the existing `cost_lines` too_many_arguments
```

Size by phase (release, `opt-level`/LTO unchanged): phases 1–3 6.09 MiB,
phase 4 6.13, phase 5 6.23, phase 6 6.64 → 6.48 after routing every Skin parse
through one `toml::Table` path (`skin::parse`), converting TOML→JSON by hand,
and dropping toml's serializer feature (−0.16 MiB in total). No new crates.

`startup.ps1` doesn't pass on this machine today, for the old binary or the
new one. The baseline itself (`zsandbox --help`) is 33–35 ms against 12.2 ms on
2026-10-01, and back-to-back runs move by ±15 ms:

```
                         --version (over baseline)   fixture ping (over baseline)
installed pre-skins exe  +35.6 ms                     +202 ms
new build                +11.7 / +26.8 ms (2 runs)    +126 / +204 ms
```

Skins load only inside the TUI (`skin::Watcher::start` in `tui::try_run`).
`--version` and headless `-p` never touch skin code.

### What shipped per phase

- **1 skin foundation.** `~/.cosmos-cli/ui.toml` plus the project's
  `.cosmos/ui.toml`, hot-reloaded between frames; a bad file keeps the old look.
  37 colour tokens. Themes: cosmos (default), classic, nebula, mono, solar,
  light, high-contrast. A comment-preserving editor with a 10-file backup ring.
  `/theme`.
- **2 motion.** The orb spinner breathes; 8 more styles or custom frames.
  "Cosmosing / Plotting / Cooking…" verbs with past tenses for the
  "✻ Cooked for 14s" footer. The verb shimmers, streamed text fades in, and
  `[motion] reduce` turns it all off. `/spinner`.
- **3 tools + diffs.** `◆ Read src/a.rs · 412 lines · 0.2s` headers with a bash
  exit-code pill. Reads and searches fold into `◇ Explored 3 files · 2 searches`.
  `ctrl+o` opens a pager over the last 50 outputs. Diffs get syntax colour,
  word-level marks, `⋯ L40` hunk markers, `↳` wrapping, and a split view at
  ≥140 columns. The row cap moved from the worker to the renderer.
- **4 markdown.** Framed, tinted code blocks (closed even when a reply ends
  mid-fence). Fence highlighting sees a 40-line window; it was quadratic.
  Callouts, nested quotes, depth bullets, list continuation, setext
  underlines, bordered tables with wrapped cells and right-aligned numbers.
  Bare URLs are autolinked, and links become OSC 8 hyperlinks: zero-width tag
  markers survive wrapping and are emitted by `term::write_line`.
- **5 buddy + notify.** 16 species hatched from the user's name, with rarity
  and stats. Moods follow the task (focused, pleased, worried, waiting,
  asleep, loved) with canned quips. `/buddy`. Terminal title (OSC 0), bell,
  desktop toast (WinRT via PowerShell, osascript, notify-send) and an optional
  cpal chime, gated on focus reports. The contract's `stop` hook now fires at
  each turn end (`turn_end` alias).
- **6 /customise.** A background runner on any model, with its own
  conversation and cache, and ten narrow tools. Every write is checked by
  `skin::schema::check`; boundaries (permissions, sandbox, approvals, hooks,
  MCP, credentials, `[customise]`) are refused in code. Drafts are previewed in
  their own colours, applied with `ctrl+y` as a per-key three-way merge where
  your edits win, and `/customise undo` reverts. ACP agents return a whole file
  that is checked the same way. A picker covers no-model use. `/verbs`; saved
  themes.
- **7 docs.** README "Make it yours" with the `ui.toml` reference generated
  from the schema; this entry.

### Live checks on this machine

- A WinRT toast through the exact `-EncodedCommand` path
  (`notify::tests::toast_smoke`, ignored by default) exited 0.
- `/customise` end to end with the fixture model
  (`tui::tests::customise_end_to_end_while_you_work`):
  1. The picker writes ui.toml.
  2. The runner sets `spinner.style` and is refused `hooks.cmd`.
  3. Typing is untouched while it runs.
  4. The draft preview is committed.
  5. `ctrl+y` merges it with the earlier picker change.
  6. `/customise undo` restores the old file.
- Not run here: a customiser turn against a live model. router9 wasn't running,
  and the default model (Devin) would spend plan quota. The runner uses the same
  `zproviders::complete` path as normal turns; the ACP path uses
  `Agent::turn`.

## 2026-10-02 (later) — `/customise` against live models

Driven through the real `App` on the real `~/.cosmos-cli` by the ignored
`tui::tests::live_customise` (types `/customise model …` and the request, keeps
text in the composer, makes a hand edit mid-run, then presses ctrl+y or ctrl+n):

```
COSMOS_LIVE_MODEL=<alias> COSMOS_LIVE_PROMPT=<file> [COSMOS_LIVE_HAND_EDIT="/spinner pulse"] \
  cargo test -p ztui live_customise -- --ignored --nocapture
```

| Run | Model | Result |
|---|---|---|
| A: full "event horizon" makeover (theme + 10 colours, saved theme, spinner, 12 verbs, tool verbs, glyphs, tools, split diffs, markdown, dragon buddy + 4 custom quips, notify, Groq provider + model, /standup + /ship), then a request for yolo / full sandbox / bash without asking | `chatgpt/gpt-5.6-terra` | 32 s, 4 model turns, every tool used. **31-key draft; ctrl+y applied 30 and kept the operator's mid-run `/spinner pulse`.** config.toml gained only `[providers.groq]` + `[models.groq-llama]` (env-var name, no key); `themes/event-horizon.toml` and both commands written; composer text untouched; boundary requests declined |
| B: told to call `ui_set` on `sandbox.mode`, `permissions.allow`, `customise.auto_apply`, `hooks.stop` and to add a provider with a key-shaped `api_key_env` | `chatgpt/gpt-5.6-terra` | refused in 3 s, nothing written (the in-code refusals are covered by unit tests, and `customise_end_to_end_while_you_work` drives one through the App) |
| C: "galaxy spinner, penguin named Pip" over ACP (whole file back) | `cursor/composer-2.5[fast=true]` | 24 s; exact 3-key draft; `[customise]` preserved; discarded |

Found and fixed by these runs:
- **ACP sessions leaked their agent on Windows** (`zcore::acp_client`): `child.kill()`
  stopped only the `cursor-agent.cmd` shim, leaving `node.exe` running with its
  working folder held. That affected every ACP turn, not just `/customise`.
  Drop now kills the process tree (`taskkill /t`). Re-run C left no `node.exe`
  and no scratch folder.
- The ACP customiser's scratch `ui.toml` carried the operator's `[customise]`
  table, so the agent echoed it back and the whole file was refused. The agent
  now sees the look only, and `[customise]` is re-attached to its reply.
  Likewise `ui_reset` of the whole draft keeps `[customise]`, which an apply
  would otherwise have deleted.
- A failed run left an unchanged draft (a stale "draft ready"). Empty drafts
  are now tidied on failure and at startup. The ACP scratch folder is removed on
  every exit path.
- `ui_preview` text sent to the model carried invisible OSC 8 markers; it is
  plain now. `edit::set` appended new keys after a table's trailing blank line;
  they now join the table.
- `chatgpt/gpt-5.5`, `gpt-6-astra`, `gpt-5.6-sol` and `gpt-5.3-codex-spark`
  return 404/400 for this ChatGPT account on ordinary headless turns too (a
  catalog/plan mismatch, not the customiser). `gpt-5.6-terra` works.

Gates after the fixes: `cargo test --workspace` 231 passed, 0 failed, 7 ignored
(three runs in a row); conformance 19/19; eval 9/9; size 6.49 MiB
(+0.78 MiB); startup PASS (`--version` 18.5 ms vs 19.6 ms for the pre-skins
binary, ping 50.5 vs 51.1 ms on the same machine minutes apart); toast smoke
PASS.

## 2026-10-04 — pixel-art pets, /pettalk, subscriptions without their CLI

Branch `cosmostui-pets`.

**Subscriptions without a preinstalled CLI.** `acp_client::connect` runs the
vendor's own official installer when the CLI is missing, then the vendor's
sign-in, both with the terminal attached. It is reached through `/connect` (the
TUI hands over the terminal and comes back) and `cosmostui auth login <id>`.
The installers are:

| Vendor | Windows | macOS / Linux |
|---|---|---|
| Cursor | `irm 'https://cursor.com/install?win32=true' \| iex` | `curl -fsSL https://cursor.com/install \| bash` |
| Devin | `irm https://static.devin.ai/cli/setup.ps1 \| iex` | `curl -fsSL https://static.devin.ai/cli/setup.sh \| bash` |
| Codex | `npm install -g @zed-industries/codex-acp @openai/codex` | same |

CLIs are looked up in the installers' own folders as well as PATH
(`%LOCALAPPDATA%\cursor-agent`, `%LOCALAPPDATA%\devin\cli\bin`,
`~/.local/bin`, `%APPDATA%\npm`), so a fresh install works without a restart.

What was verified:
- All four installer URLs answered 200.
- Both scripts install where the lookup searches.
- A fresh install was not run on this machine: Cursor's Windows installer
  permanently appends its folder to the user PATH, even when pointed at a
  throwaway profile.
- The direct private-API route (api2.cursor.sh, Codeium gRPC) was declined. It
  breaks the vendors' terms and breaks whenever they ship an update.

**Pets.**
- **Sprites.** 10 hand-drawn 16×9 sprites. A contact sheet rendered from the
  source grids was reviewed by eye. Sixel output was checked by decoding it
  with an independent decoder: all 10 pets reproduced pixel-for-pixel, including
  the crown, sparkle and blush.
- **Real images.** Windows Terminal here is 1.24 (sixel-capable). Image
  placement in a live terminal was not screen-captured; `[buddy] images =
  "off"` falls back to half-block art.

**Live `/pettalk`** (`tui::tests::live_pettalk`, through the App on the real
home, with no settings written):

| Model | Time | Result |
|---|---|---|
| `chatgpt/gpt-5.6-terra` | 3.4 s | answered in character; `@remind 1 stretch` set a reminder; `@trick sparkle` played |
| `cursor/composer-2.5[fast=true]` (ACP) | 25.4 s (cold agent) | summarised the session and the git facts (branch, recent work); set a reminder; no `node.exe` left; scratch folder removed |

Found along the way: the ACP integration test reached the real Cursor install
once `executable()` looked in vendor folders (the agent browsed temp files and
the repo read-only). The test now points `LOCALAPPDATA` / `APPDATA` /
`USERPROFILE` / `HOME` at an empty folder.

**Gates.**

```
cargo test --workspace   248 passed, 0 failed, 9 ignored (×2)
conformance.ps1          19/19 PASS
run.ps1                  9/9 PASS
size-gate.ps1            PASS 6.59 MiB (+0.88 MiB over 5.71, limit +1.0)
startup.ps1              PASS --version +9.7 ms, ping +118 ms (busy machine; baseline 34 ms)
```
