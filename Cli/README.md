# Cosmos CLI family

Three coding-agent harnesses — `zcli` (Rust TUI), `zdev` (TypeScript/bun),
`zagy` (Go) —
that share one memory and one session vault through the Meridian bridge. You can
start a task in `zcli`, resume it in `zagy`, and read it back in `zdev` without
losing context.

## Layout

```
Cli/
  CONTRACT.md        the shared spec every harness implements
  RESEARCH.md        upstream research notes (codex, pi, grok-build, opencode)
  config.example.toml
  contracts/tools.json  canonical tool schema (byte-identical on all three)
  meridian-bridge/   Rust memory/session bridge + Python and Go clients
  rust-tui/          zcli + zcore/zproto/zproviders/ztools/ztui/zsandbox
  zdev/              zdev (TypeScript + Ink/React, bun; single compiled exe)
  go-ssh/            zagy (stdlib-only Go)
  eval/              event schema, conformance gate, eval tasks
```

## Build

```
cd rust-tui        && cargo build --workspace        # zcli, zcli-welcome, zsandbox
cd meridian-bridge && cargo build                    # meridian + clients tests
cd zdev            && bun install && bun run build   # zdev → dist/zdev
cd go-ssh          && go build -o zagy .             # zagy
```

## Use

All binaries are installed globally in a directory on `$PATH` (for example
`$HOME/.local/bin`):
`zcli`, `zagy`, `zdev`, `meridian`, `zsandbox` — all single-file executables.
`zdev-ui` remains as a compat alias for `zdev ui` (the TUI is in-process now).

```
zcli -p "fix the failing test"        # headless
zcli                                  # interactive TUI (ratatui): /model /plan /yolo …
zcli session list|export|branch|tree
zcli memory search|add [--type t]
zcli tools dump                       # canonical contracts/tools.json
zcli acp                              # stdio JSON-RPC (ACP) server
zdev -p "ping" --model r9             # headless bun harness
zdev                                  # Ink/React TUI (monochrome, zcli's galaxy)
zdev login | logout                   # ChatGPT Plus/Pro sign-in (shared with zcli)
zdev models [provider]                # models you can use now
zagy                                  # interactive TUI (bubbletea)
```

Live models route through 9Router (`ROUTER9_API_KEY` is set at User scope):
`--model r9` = kr/claude-sonnet-4.5, `r9free` = kc/kilo-auto/free,
`r9gem` = ag/gemini-3.8-flash. `fixture` needs no key. 9Router is also in
both CLIs' BYOK lists as `router9/<route>`.

zdev and zcli share one setup. Keys use macOS Keychain, Linux
`credentials.json` fallback, or the platform credential store; the ChatGPT
sign-in is in `credentials.json`, and
both read the same models.dev catalog and `live_models.json`. A key added in
either CLI works in the other. zdev's TUI streams replies into scrollback
paragraph by paragraph, so long answers never tear the screen. It supports
`/model` (live lists, effort), `/connect`, `/login`, `/logout`, `/usage`,
`/context`, `/resume`, `/init`, `/review`, `/diff`, `/copy`, custom commands
(`.zcli/commands`, `.claude/commands`), `@file` fuzzy mentions, `!cmd`,
`#note`, `ctrl+space` dictation, `ctrl+o` full tool output, and `shift+tab`
modes. The first `zdev` run plays the same full-screen onboarding as zcli
(galaxy fly-through, "welcome to cosmos", name, keys with live model lists,
how-to). Replay it with `zdev onboarding` or `/onboarding`.

`zdev` and `zagy` accept the same commands and flags
(`--output text|json|json-stream`, `--mode plan|agent`, `--allow`, `--yolo`,
`--no-sandbox`, `--model`, `--max-turns`, `--max-usd`, `--cwd`, `--resume`,
`--approve-plan`, `--allow-escalation`).

## zcli interactive UI

Modelled on Devin CLI. It renders inline (no alternate screen): the banner,
your turns, streamed replies and tool results go into the terminal's own
scrollback, so native scroll, search and copy all work. A live region redraws
underneath it: the line still being written, running tools, the ruled `❯`
composer, the status line, and `/` menus that open below it.

- Replies stream token by token and are committed a line at a time
  (`ztui/src/markdown.rs`). Tables, lists, quotes and links are rendered, and
  fenced code is highlighted by tree-sitter (13 grammars).
- `edit`/`write` calls show a real before/after diff with line numbers.
- `/model` opens a fuzzy picker. `←`/`→` sets reasoning effort, which is sent as
  `reasoning_effort` to OpenAI-compatible providers.
- `/usage`, `/context`, `/mode`, `/approve`, `/session`, `/clear`, `/help`.
  `↑`/`↓` scroll prompt history, which is saved to `zcli_history.jsonl`. Pastes
  over 8 lines collapse to a placeholder. `ctrl+c` interrupts, then clears the
  input, then quits on a second press.
- BYOK: `/connect` (or `zcli auth login <provider>`) stores keys for 30
  models.dev providers in the OS keychain. `/login` signs in with a ChatGPT
  Plus/Pro plan. Models are addressed as `provider/model`.
- `ctrl+space` dictates the prompt. It records from the mic and transcribes with
  OpenAI or Groq, or offline with Windows speech recognition.

First run: `zcli` starts `zcli-welcome`, a separate executable installed next
to it. It takes over the whole terminal. A short sound plays as you fly
through a spiral galaxy, and the stars re-form as "welcome to cosmos" in
braille. Then it asks for your name and sets up keys: ChatGPT sign-in,
OpenRouter, or any of the other providers. Each key is checked against the
provider's API, and that provider's live model list opens so you can pick your
default. The list is cached in `live_models.json`, so `/model` shows it too.
A how-to screen comes last. When you launch, the terminal is cleared so only
zcli is on screen. The flow writes `zcli-onboarded.json`, and zcli deletes
`zcli-welcome`, so none of that code ships in the binary you run every day.
`ZCLI_NO_ONBOARDING=1` skips it, `ZCLI_QUIET=1` mutes it, and
`ZCLI_KEEP_WELCOME=1` keeps the executable (for development).

Stack: Devin CLI's crate list, read from its binary, is crossterm + tokio +
pulldown-cmark/mdstream + tree-sitter-highlight + nucleo + similar, with
Dioxus/Blitz as its UI layer. zcli uses the same crossterm, pulldown-cmark,
tree-sitter-highlight, nucleo-matcher and similar. It paints the UI with its own
~200-line inline renderer (`ztui/src/term.rs`) instead of a DOM/CSS engine.
That's why its binary is 18 MB and Devin's is 180 MB. The renderer's scrollback
behaviour is tested against the alacritty terminal emulator.

## Shared state

All harnesses read `COSMOS_CLI_HOME` (default `~/.cosmos-cli`):
`sessions/<id>.jsonl` event logs, `worktrees/`, `skills/`. Memory lives in the
Meridian vault (`MERIDIAN_VAULT`, default `~/.meridian/vault`),
append-only markdown bullets (`decision|preference|map|correction|playbook`)
searched over BM25 — never touches Meridian's app vault.

## Safety

`bash` runs through `zsandbox`: Windows AppContainer (restricted-token fallback),
Job-object limits, workspace path confinement, network deny by default
(`--net allow` opts in). File edits require hashline anchors from `read`;
stale-file and escape attempts are rejected. Outbound HTTP is limited to
configured provider/MCP hosts. Tools follow `[permissions]` in config.toml; without
that table (a fresh install), read-only tools run and anything that edits files
or runs commands asks first.

## Verify

```
powershell -File eval\conformance.ps1   # 15 checks across all harnesses
powershell -File eval\run.ps1           # eval tasks (fixture, deterministic)
cargo test --workspace                  # in rust-tui/
bun test                                # in zdev/
go test ./...                           # in go-ssh/
```

With no provider key configured every harness uses the deterministic `fixture`
provider (`ping`→`pong`, `tool:NAME {json}`→one tool call), so all checks run
offline. See `EVIDENCE.md` for the recorded run.
