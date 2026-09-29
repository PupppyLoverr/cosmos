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
