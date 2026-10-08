# Cosmos CLI family

Three coding-agent harnesses — `cosmostui` (Rust TUI), `zdev` (TypeScript/bun),
`zagy` (Go) —
that share one session vault and one memory format. You can
start a task in `cosmostui`, resume it in `zagy`, and read it back in `zdev` without
losing context. (`cosmostui` was `zcli`; invoking it as `zcli` still works and
prints a one-line deprecation notice.)

## Layout

```
Cli/
  CONTRACT.md        the shared spec every harness implements
  RESEARCH.md        upstream research notes (codex, pi, grok-build, opencode)
  config.example.toml
  contracts/tools.json  canonical tool schema (byte-identical on all three)
  meridian-bridge/   Rust memory/session bridge + Python and Go clients (zdev/zagy)
  rust-tui/          cosmostui + zcore/zproto/zproviders/ztools/ztui/zsandbox
  zdev/              zdev (TypeScript + Ink/React, bun; single compiled exe)
  go-ssh/            zagy (stdlib-only Go)
  eval/              event schema, conformance gate, eval tasks
```

## Build

```
cd rust-tui        && cargo build --workspace        # cosmostui, cosmostui-welcome, zsandbox
cd meridian-bridge && cargo build                    # meridian + clients tests
cd zdev            && bun install && bun run build   # zdev → dist/zdev
cd go-ssh          && go build -o zagy .             # zagy
```

## Install

`cosmostui` is one self-contained executable (~6 MiB, no runtime deps):

```
copy rust-tui\target\release\cosmostui.exe %USERPROFILE%\bin\        # Windows
cp   rust-tui/target/release/cosmostui     ~/.local/bin/             # Linux/macOS
copy rust-tui\target\release\cosmostui.exe %USERPROFILE%\bin\zcli.exe  # optional: old alias
```

The `zcli.exe` copy keeps old scripts working — argv0 `zcli` prints a one-line
deprecation notice and continues. `cosmostui-welcome.exe` can sit next to it;
it runs once on first launch and is then deleted. Shell completions:
`cosmostui completion bash|zsh|fish|powershell`.

## Use

All binaries are installed globally in a directory on `$PATH` (for example
`$HOME/.local/bin` or `%USERPROFILE%\bin`):
`cosmostui`, `zagy`, `zdev`, `meridian`, `zsandbox` — all single-file executables.
`zdev-ui` remains as a compat alias for `zdev ui` (the TUI is in-process now).

```
cosmostui -p "fix the failing test"   # headless
cosmostui                             # interactive TUI: /model /plan /yolo …
cosmostui session list|export|branch|tree
cosmostui memory search|add [--type t]
cosmostui tools dump                  # canonical contracts/tools.json
cosmostui acp                         # stdio JSON-RPC (ACP) server
cosmostui auth login chatgpt|devin|cursor|codex   # subscription sign-ins
zdev -p "ping" --model r9             # headless bun harness
zdev                                  # Ink/React TUI (monochrome, zcli's galaxy)
zdev login | logout                   # ChatGPT Plus/Pro sign-in (shared with cosmostui)
zdev models [provider]                # models you can use now
zagy                                  # interactive TUI (bubbletea)
```

Live models route through 9Router (`ROUTER9_API_KEY` is set at User scope):
`--model r9` = kr/claude-sonnet-4.5, `r9free` = kc/kilo-auto/free,
`r9gem` = ag/gemini-3.8-flash. `fixture` needs no key. 9Router is also in
both CLIs' BYOK lists as `router9/<route>`.

zdev and cosmostui share one setup. Keys use macOS Keychain, Linux
`credentials.json` fallback, or the platform credential store; the ChatGPT
sign-in is in `credentials.json`, and
both read the same models.dev catalog and `live_models.json`. A key added in
either CLI works in the other. zdev's TUI streams replies into scrollback
paragraph by paragraph, so long answers never tear the screen. It supports
`/model` (live lists, effort), `/connect`, `/login`, `/logout`, `/usage`,
`/context`, `/resume`, `/init`, `/review`, `/diff`, `/copy`, custom commands
(`.cosmos/commands`, `.claude/commands`), `@file` fuzzy mentions, `!cmd`,
`#note`, `ctrl+space` dictation, `ctrl+o` full tool output, and `shift+tab`
modes. The first `zdev` run plays the same full-screen onboarding as cosmostui
(galaxy fly-through, "welcome to cosmos", name, keys with live model lists,
how-to). Replay it with `zdev onboarding` or `/onboarding`.

`cosmostui` also drives subscription plans you already pay for through their
own CLIs over ACP: `cosmostui auth login devin` (Devin), `cursor` (Cursor
`cursor-agent`), `codex` (`codex-acp`). The vendor's agent loop does that
turn's work; cosmostui renders it, holds the permissions prompt, and keeps the
session log. The plan's real model list is pulled from the agent's own
`session/new` reply (its `models` block, or a `model` config select — Devin's
dialect) at sign-in, `/connect`, onboarding, and `cosmostui models <agent>`;
it's cached in `live_models.json` so `/model` shows it too, and `<id>/default`
always means "the plan decides".

`zdev` and `zagy` accept the same commands and flags
(`--output text|json|json-stream`, `--mode plan|agent`, `--allow`, `--yolo`,
`--no-sandbox`, `--model`, `--max-turns`, `--max-usd`, `--cwd`, `--resume`,
`--approve-plan`, `--allow-escalation`).

## cosmostui interactive UI

Modelled on Devin CLI. It renders inline (no alternate screen): the banner,
your turns, streamed replies and tool results go into the terminal's own
scrollback, so native scroll, search and copy all work. A live region redraws
underneath it: the line still being written, running tools, the ruled `❯`
composer, the status line, and `/` menus that open below it.

- Replies stream token by token and are committed a line at a time
  (`ztui/src/markdown.rs`). Tables, lists, quotes and links are rendered, and
  fenced code is highlighted by a built-in highlighter (tree-sitter with 13
  grammars is still available via the `highlight` cargo feature).
- `edit`/`write` calls show a real before/after diff: line numbers, syntax
  colour, the changed words highlighted, long lines wrapped with `↳`, and old
  and new side by side on wide terminals.
- Tool calls carry their facts in the header (`◆ Read src/a.rs · 412 lines ·
  0.2s`, `◆ Bash cargo test · exit 101 · 3.4s`). Runs of reads and searches fold
  into one `◇ Explored 3 files · 2 searches` block. `ctrl+o` opens the full
  output of the last 50 tool calls in a pager.
- The thinking line is an orb that breathes, a verb that shimmers ("Cosmosing…",
  "Plotting…", "Cooking…"), and a "✻ Cooked for 14s" footer when the turn ends.
  Streamed text fades in.
- Markdown gets framed code blocks, callouts (`> [!NOTE]`), bordered tables
  that wrap, nested quotes and lists, and clickable OSC 8 links where the
  terminal supports them.
- A pixel-art pet sits beside the composer, reacts as you work, levels up
  with you, and talks back with `/pettalk` (see below). When a turn ends or
  needs you while you're in another window, cosmostui sets the terminal
  title, rings the bell, and shows a desktop notification.
- `/model` opens a fuzzy picker. `←`/`→` sets reasoning effort, which is sent as
  `reasoning_effort` to OpenAI-compatible providers.
- `/usage`, `/context`, `/mode`, `/approve`, `/session`, `/tree`, `/compact`,
  `/undo`, `/skillify`, `/clear`, `/help`. The status line carries the §8
  ledger (turn · in · cache_r · cache_w · out · $ · Σ · ctx%).
  `↑`/`↓` scroll prompt history, which is saved to `history.jsonl`. Pastes
  over 8 lines collapse to a placeholder. `ctrl+c` interrupts, then clears the
  input, then quits on a second press.
- Subscriptions: `/connect cursor`, `/connect devin` or `/connect codex` runs
  that vendor's own agent on your plan. If its CLI isn't installed yet,
  cosmostui runs the vendor's official installer (cursor.com/install,
  static.devin.ai/cli/setup, or npm for Codex), then the vendor's sign-in,
  and comes back with the plan's model list. `cosmostui auth login <id>`
  does the same from a shell.
- BYOK: `/connect` (or `cosmostui auth login <provider>`) stores keys for 30
  models.dev providers in the OS keychain. `/login` signs in with a ChatGPT
  Plus/Pro plan. Models are addressed as `provider/model`.
- `ctrl+space` dictates the prompt. It records from the mic and transcribes with
  OpenAI or Groq, or offline with Windows speech recognition.

First run: `cosmostui` starts `cosmostui-welcome`, a separate executable
installed next
to it. It takes over the whole terminal. A short sound plays as you fly
through a spiral galaxy, and the stars re-form as "welcome to cosmos" in
braille. Then it asks for your name and sets up keys: ChatGPT sign-in,
OpenRouter, or any of the other providers. Each key is checked against the
provider's API, and that provider's live model list opens so you can pick your
default. The list is cached in `live_models.json`, so `/model` shows it too.
A how-to screen comes last. When you launch, the terminal is cleared so only
cosmostui is on screen. The flow writes `onboarded.json`, and cosmostui deletes
`cosmostui-welcome`, so none of that code ships in the binary you run every day.
`COSMOS_NO_ONBOARDING=1` skips it, `COSMOS_QUIET=1` mutes it, and
`COSMOS_KEEP_WELCOME=1` keeps the executable (for development); the old
`ZCLI_*` spellings are still read.

Stack: Devin CLI's crate list, read from its binary, is crossterm + tokio +
pulldown-cmark/mdstream + tree-sitter-highlight + nucleo + similar, with
Dioxus/Blitz as its UI layer. cosmostui uses crossterm, pulldown-cmark,
nucleo-matcher and similar, and paints the UI with its own
~200-line inline renderer (`ztui/src/term.rs`) instead of a DOM/CSS engine.
The release binary is ~6 MiB and Devin's is 180 MB. The renderer's scrollback
behaviour is tested against the alacritty terminal emulator.

## Make it yours

Everything cosmostui looks like, and the harmless parts of how it behaves, come
from one file: `~/.cosmos-cli/ui.toml`, with a project's `.cosmos/ui.toml`
layered on top. Edit it while cosmostui runs. The new look swaps in between two
frames, and a file that doesn't parse keeps the old look and says why. Every
write cosmostui makes keeps the previous file in a ring of ten backups
(`ui.toml.bak.1`…).

```toml
[theme]
name = "nebula"                 # cosmos, classic, nebula, mono, solar, light, high-contrast
[theme.colors]
accent = "#c792ea"

[spinner]
style = "comet"
verbs = ["Warping:Warped", "Folding space:Folded space"]

[diff]
layout = "split"

[buddy]
species = "axolotl"
name = "Pip"

[notify]
desktop = "always"
sound = true
```

Quick commands write the same file: `/theme`, `/spinner`,
`/verbs add Present:Past`, `/buddy …`, `/notify [test]`.
`/customise reset classic` brings back the look cosmostui had before skins (a
test checks that it renders exactly as before).

### /customise

`/customise` on its own opens a picker of the settings people change most
(theme, spinner, tool calls, diffs, markdown, links, buddy, species, motion,
notifications). Each `←`/`→` applies at once.

`/customise <anything>` hands the request to a model, for example "make it
purple and use Dune verbs", "add Groq with llama 3.3", or "a /standup command".
It uses the session's model by default; `/customise model <alias>` picks a
cheaper one. The customiser:

- **Runs in the background.** It has its own thread, conversation and prompt
  cache, so you keep typing and running turns. A strip above the composer shows
  what it's doing. `/customise log` lists every step, `/customise stop` ends it,
  and its spend is shown separately in the ledger (`· mod $…`).
- **Has a narrow tool set and nothing else:** `ui_get`, `ui_set`, `ui_preview`,
  `ui_apply`, `ui_reset`, `theme_save`, `provider_add`, `model_add`,
  `subscription_enable`, `command_add`.
- **Writes a draft (`ui.draft.toml`).** When it finishes you see the draft
  rendered in its own colours. `ctrl+p` tries it on the live UI (spinner,
  pet, composer, status), `ctrl+y` applies it and `ctrl+n` discards it.
  Applying is a per-key three-way merge: if you changed a key yourself while it
  worked, your value is kept. `/customise undo` steps back through the backups.
  `[customise] auto_apply = true` applies each change as it validates.
- **Can't cross hard boundaries.** Every write is checked against the schema
  below (path, type, allowed values, range, colour, width). Permissions,
  sandbox, approvals, hooks, MCP, credentials and the customiser's own
  `[customise]` limits are refused in code. New providers and models only add
  their own table to `config.toml` and name the environment variable for a key;
  you add keys yourself with `/connect`.
- **Works with any model.** Subscription agents (Cursor, Devin, Codex) run their
  own tools, so they are asked for the finished file from an empty scratch
  folder, and that file goes through the same checks. With no model connected,
  you get the picker.

### Pets, /pettalk and notifications

Your pet is solid pixel art that sits on the composer's right edge. It hatches
from your name as one of 10 creatures (fox, cat, axolotl, owl, penguin, dragon,
slime, ghost, octopus, robot), with a rarity and stats. In terminals that can
show real images (sixel: Windows Terminal 1.22+, WezTerm, iTerm2, foot; kitty
protocol: kitty, Ghostty), the `/buddy` card and `/pettalk` replies draw it as
an image. Elsewhere they draw it in coloured half-blocks.

- **It watches the task.** It looks focused while a turn runs and hops when
  the turn lands. It frowns at failed tools, sweats at red tests and cheers
  when they go green. It waits with you at permission prompts and falls asleep
  when you're idle. Its eyes change with each mood.
- **It grows.** Finished turns, red tests fixed, pats and chats earn XP, and
  using cosmostui on consecutive days builds a streak (saved in `pet.json`).
  Levels unlock rosy cheeks (3), a ✦ sparkle (5), a crown (10) and a shiny
  coat (15).
- **It helps.** It suggests a break after `break_mins` of work. When you come
  back to the terminal it says what happened while you were away. It also
  keeps reminders, which ring the bell and send a desktop notification.
- **You can talk to it.** `/pettalk <anything>` answers in character with
  the model you're using right now. It runs as its own side conversation, so
  your main transcript and prompt cache never see it. It knows the session
  (recent requests, the last reply, what's running, the cost) and the repo
  (branch, changed files, last commit). It can act on what you say: set a
  reminder ("remind me in 20 minutes to stretch"), do a trick, take a new name
  or form, or hand a look-and-feel request to `/customise`, which applies all
  of its boundaries. `/pettalk stop` cancels.

`/buddy` shows the card. The subcommands are
`/buddy pet | trick <hop|spin|sparkle|nap> | name <n> | species [name] | reroll | mini | full | quiet | off`.
`[buddy.colors]` recolours any part of the pixel art.

Notifications only interrupt when the terminal reports it has lost focus.
Terminals that never report focus notify only for turns of at least
`min_secs`. The desktop notification is a WinRT toast through PowerShell,
`osascript` on macOS, or `notify-send` on Linux. `sound = true` plays a short
chime (with the `voice` feature). The `stop` hook (alias `turn_end`) fires at
the end of every turn for anything else.

### ui.toml reference

Generated from the code (`cargo test -p ztui dump_schema -- --ignored
--nocapture`). `/customise schema` prints the same list.

```text
[theme]
  colors.<token>: colour; tokens: text, accent, success, warn, error, shell, dim, rule, ghost, user_bg, diff_add_bg, diff_del_bg, diff_add_word, diff_del_word, code, code_bg, heading, heading_sub, link, quote, badge_fg, logo_bright, logo_mid, logo_dim, syntax.base, syntax.comment, syntax.keyword, syntax.string, syntax.escape, syntax.number, syntax.function, syntax.type, syntax.tag, syntax.attribute, syntax.operator, syntax.variable, syntax.punctuation — override one colour token: "#rrggbb", "#rgb" or an ANSI name
  name: string = "cosmos" — one of "cosmos" | "classic" | "nebula" | "mono" | "solar" | "light" | "high-contrast" — colour theme (built-in or saved with theme_save)
[spinner]
  footer: bool = true — "✻ Cooked for 12s" after each turn
  footer_glyph: string = "✻" — glyph before that footer
  fps: integer = 12 — 1..=60 — spinner frames per second
  frames: list of strings = [] — custom spinner frames; non-empty replaces style
  hint: string = "esc to interrupt" — hint at the end of the busy line
  shimmer: bool = true — a highlight sweeping across the verb
  show_elapsed: bool = true — seconds in the busy line
  show_tokens: bool = true — streamed tokens in the busy line
  style: string = "orb" — one of "orb" | "hop" | "sparkle" | "heart" | "twinkle" | "braille" | "moon" | "comet" | "pulse" | "galaxy" | "dots" | "line" — the thinking spinner
  tool_verbs.<tool>: string — verb shown while a tool runs (read, grep, glob, ls, bash, edit, write, task, …)
  verbs: list of strings = ["Cosmosing:Cosmosed","Plotting:Plotted","Cooking:Cooked","Orbiting:Orbited","Stargazin… — "Present:Past" thinking verbs, one picked per turn ("Cooking:Cooked")
[motion]
  reduce: bool = false — no animation at all
  reveal: bool = true — streamed text fades in
  reveal_ms: integer = 160 — 0..=2000 — how long the fade-in takes
[tools]
  group: bool = true — fold runs of reads/searches into one "Explored" block
  keep_rows: integer = 4 — 0..=50 — output rows shown under a finished tool (ctrl+o shows all)
  keep_rows_search: integer = 3 — 0..=50 — rows shown under search tools
  show_elapsed: bool = true — how long each tool took
  style: string = "cosmos" — one of "cosmos" | "classic" — tool-call layout
[diff]
  context: integer = 2 — 0..=20 — unchanged lines around each change
  layout: string = "auto" — one of "auto" | "unified" | "split" — split = side by side; auto splits on wide terminals
  max_rows: integer = 40 — 1..=2000 — diff rows shown before "N more lines"
  style: string = "cosmos" — one of "cosmos" | "classic" — diff layout
  syntax: bool = true — syntax colour in diffs
  word_diff: bool = true — highlight the changed words
  wrap: bool = true — wrap long diff lines instead of cutting them
[markdown]
  bullets: list of strings = ["•","◦","▸","▹"] — list bullets by nesting depth
  callouts: bool = true — render > [!NOTE] / TIP / WARNING blocks
  code_bg: bool = true — tint code blocks
  code_frame: bool = true — box code blocks with a language label
  code_numbers: bool = false — line numbers in code blocks
  h1_rule: bool = true — a rule under top-level headings
  links: string = "auto" — one of "auto" | "osc8" | "inline" — osc8 = clickable hyperlinks; inline = show the URL
  style: string = "cosmos" — one of "cosmos" | "classic" — markdown rendering
  table_borders: bool = true — box-drawn tables with wrapped cells
[glyphs]
  elbow: string = "⎿" — glyph before tool output
  group: string = "◇" — glyph before a folded group
  tool: string = "◆" — glyph before each tool call
  wrap: string = "↳" — glyph on wrapped diff lines
[buddy]
  break_mins: integer = 50 — 0..=480 — nudge to take a break after this many minutes of work (0 = never)
  colors.<part>: "#rrggbb"; parts: outline, body, shade, belly, accent, cheeks, details, eyes — recolour the pet's pixel art
  enabled: bool = true — show the companion
  images: string = "auto" — one of "auto" | "sixel" | "kitty" | "off" — real images of the pet where the terminal supports them (sixel/kitty)
  lines.<kind>: list of strings; kinds: hello, done, long_done, error, tests_pass, tests_fail, waiting, compact, pet, sleep, degraded, break, level — custom quips for a moment ({name} = the pet)
  name: string = "" — "" = a generated name
  quip_secs: integer = 6 — 1..=60 — how long a quip stays up
  quips: string = "builtin" — one of "builtin" | "off" — one-liners when things happen
  seed: integer = 0 — 0..=1000000 — change to hatch a different buddy
  size: string = "full" — one of "full" | "mini" | "off" — full = sprite beside the input; mini = face in the status line
  species: string = "" — one of "" | "fox" | "cat" | "axolotl" | "owl" | "penguin" | "dragon" | "slime" | "ghost" | "octopus" | "robot" — "" = hatched from the user's name
[notify]
  bell: string = "unfocused" — one of "unfocused" | "always" | "never" — terminal bell
  desktop: string = "unfocused" — one of "unfocused" | "always" | "never" — desktop notification
  events: list of strings = ["done","waiting","error"] — one of "done" | "waiting" | "error" — which moments notify
  min_secs: integer = 15 — 0..=3600 — turns shorter than this never notify
  sound: bool = false — a short chime instead of the bell
  title: bool = true — turn state in the terminal title
```

`[customise]` (`model`, `auto_apply`, `max_turns` = 12, `max_usd` = 0.25) is
yours to set. The customiser can't write it.

## Shared state

All harnesses read `COSMOS_CLI_HOME` (default `~/.cosmos-cli`):
`sessions/<id>.jsonl` event logs, `worktrees/`, `skills/`, `world/`.
Memory is append-only markdown bullets
(`decision|preference|map|correction|playbook`) searched over BM25. cosmostui
keeps it in-process at `COSMOS_CLI_HOME/memory` — set `COSMOS_MEMORY_DIR`, or
point `MERIDIAN_VAULT` at the shared vault, and zagy/zdev/meridian read the same
files (the bullet format is the bridge's). Optional Supermemory sync only
activates when `SUPERMEMORY_API_KEY` is set.

## Safety

`bash` runs through `zsandbox`, which tries three tiers in order: Windows
AppContainer (workspace file confinement, network deny by default —
`--net allow` opts in), a restricted token at low integrity (no network
block), then Job-object limits only (time/memory/process count — no
isolation). Anything below AppContainer is reported as `sandbox:"degraded"`
with the reason in `session_start` and every bash result;
`COSMOS_SANDBOX_TIER=appcontainer|restricted|job` pins a tier. File edits
require hashline anchors from `read`;
stale-file and escape attempts are rejected. Outbound HTTP is limited to
configured provider/MCP hosts. Tools follow `[permissions]` in config.toml; without
that table (a fresh install), read-only tools run and anything that edits files
or runs commands asks first. `write`/`edit` refuse cosmostui's own binary,
`CONTRACT.md` and sandbox policy paths unless `--allow-self-mutate`.

## Verify

```
powershell -File eval\conformance.ps1   # 19 checks across all harnesses
powershell -File eval\run.ps1           # eval tasks (fixture, deterministic)
powershell -File eval\size-gate.ps1     # cosmostui.exe ≤ 8 MiB and ≤ +1 MiB over 5.71
powershell -File eval\startup.ps1       # --version latency vs a baseline exe
cargo test --workspace                  # in rust-tui/
bun test                                # in zdev/
go test ./...                           # in go-ssh/
```

With no provider key configured every harness uses the deterministic `fixture`
provider (`ping`→`pong`, `tool:NAME {json}`→one tool call), so all checks run
offline. See `EVIDENCE.md` for the recorded run.
