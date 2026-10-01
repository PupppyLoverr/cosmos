# CONTRACT.md — Cosmos CLI family wire contract

Version 0. All three harnesses — `cosmostui` (Rust; formerly `zcli`), `zdev` (TypeScript/bun), `zagy` (Go) — conform to
this document byte-for-byte on every surface it names. Where a behavior is not listed, the
harness is free to choose; where it is listed, deviating is a bug.

## Contract system snippet

The block below is embedded verbatim in `meridian-bridge` (`include_str!`) and is layer 1 of
every prompt's stable prefix:

<!-- BEGIN SYSTEM -->
You are Cosmos, a coding agent running in the operator's terminal. You work through tools.
Rules: prefer reading before writing; use `read` anchors for every `edit`; never guess file
contents; run `bash` only inside the sandbox; ask before anything the permission table marks
`ask`; keep responses short and factual; when the task is done, say so and stop.
<!-- END SYSTEM -->

## 1. Runtime home and config

`COSMOS_CLI_HOME` (env) defaults to `~/.cosmos-cli`. It contains:

```
config.toml      sessions\<id>.jsonl      worktrees\<session>\<task_id>\
skills\<name>\SKILL.md
```

`Cli\config.example.toml` is the reference config. Providers: `kind = "openai" | "anthropic"
| "google"`, a `base_url`, and `api_key_env` naming the environment variable that holds the
key. Keys are read from the environment only — never stored. Any `kind = "openai"` provider
with any `base_url` must work (covers Ollama, llama.cpp, vLLM, LM Studio, OpenRouter, xAI).
`[models.<alias>]` maps `provider`, `id`, `context`, `price {input, cache_read, cache_write,
output}` (USD per 1M tokens). `[permissions] allow`/`ask` name tools; anything else is denied.
`[subagents] max_parallel` caps `task` concurrency. `[tools.bash] shell = "auto"` picks
Use the platform's POSIX shell when available; Windows may use PowerShell non-interactively.
`[bridge] large_read_lines` (default 200) and `summarizer_model` (empty = deterministic
extractor). `[[hooks]] event matcher command` registers shell hooks.
Project MCP servers are discovered from `.mcp.json` in the working directory. User-level
MCP configuration is read from `~/.cosmos-cli/mcp.json` when present, using the
`{"mcpServers":{"name":{"command":"…","args":[],"env":{}}}}` shape.

## 2. Tools

These are the only tools; their JSON schemas are byte-identical across harnesses (`tools dump`
output must match byte-for-byte). A harness may send a core subset on the wire and serve the
rest as *deferred* tools: `tool_search` returns their schemas as tool-result text and the model
calls them through `mcp_call {server: "cosmos", tool, arguments}`; permissions apply to the
inner tool. cosmostui sends `read edit write bash grep glob tool_search mcp_call` (plus
`submit_plan` for a session that starts in plan mode) and defers the rest.

File tools:

- `read {path, start?, end?}` — emits `file_sha256=<64hex> lines=<N>` then lines as
  `<n>:<h>|<text>`; `h` = first 6 hex chars of SHA-256 over the line bytes with the trailing
  `\r\n`/`\n` stripped.
- `edit {path, file_sha256?, edits:[{start:"<n>:<h>", end:"<n>:<h>", new_text}]}` — rejects the
  whole call if any anchor or the file hash mismatches; rejection returns `STALE` plus fresh
  anchors for ±5 lines around each failed anchor. Applies bottom-up, preserves the file's
  line endings, writes atomically (temp + rename).
- `write {path, content}` — new files allowed; overwriting requires the file was `read` this
  session and `file_sha256` still matches, else `STALE`.
- `bash {command, timeout_s=30, wait?: now|next_write|exit, from_offset?}` — always
  through the sandbox (§5). `wait=now` returns `{task_id,pid,log_path,offset}` immediately.
  `from_offset` reads only new bytes from the persisted log. Output is middle-out truncated
  (head 24 + tail 24 lines), fenced as `<<<UNTRUSTED_SHELL … >>>`, and always includes
  `exit_code`, `log_path`, `omitted_chars`. Grep-style exit 1 with empty stdout is
  `no matches`, not a harness failure.
- `grep {pattern, path?, glob?}` — respects `.gitignore`, output `path:line:text`, cap 200.
- `glob {pattern}`; `ls {path}`.

Harness tools:

- `memory_search {query, k=5, scope?}`, `memory_write {type, text, scope}` — markdown bullets
  in the Meridian vault format, BM25-searched (cosmostui keeps them in-process under
  `COSMOS_CLI_HOME/memory`, or `$MERIDIAN_VAULT/memory` when that is set; with
  `SUPERMEMORY_API_KEY` set it also mirrors to Supermemory, falling back to local results).
- `context_remaining {}` — JSON `{tokens_in, tokens_budget, percent, cache_warm}`.
- `new_context {}` — force L5 compact and reseed from `world.md`.
- `task {prompt, write:false, worktree:false}` — subagents (§10).
- `request_escalation {capability: "network"|"write_outside_workspace"|"unsandboxed", reason}`.
- `submit_plan {plan}` — only in plan mode.
- `tool_search {query}`, `mcp_call {server, tool, arguments}` — deferred harness tools (server
  `cosmos`) plus any configured MCP servers.

Every tool result is capped at 30,000 characters and ends with an explicit truncation notice
when capped. Nothing else ships in v0.

## 3. Modes

- `plan`: read/grep/glob/ls/memory_search/submit_plan only. `/approve` (TUI) or
  `--approve-plan` (headless) switches to `agent` with the plan pinned as the next user
  message.
- `agent`: follows `[permissions]`; in headless mode an `ask` counts as deny unless the tool
  is in `--allow`.
- `accept-edits`: writes and edits auto-allow; other tools follow `[permissions]`.
- `yolo`: only via `--yolo`; auto-allows every tool; sandbox stays on unless `--no-sandbox`.
  Shift+Tab cycles `plan → agent → accept-edits → yolo`. Sandbox policy (`--sandbox
  read-only|workspace-write|full`) and approval policy (`--ask on-request|never|untrusted`)
  are independent dials.

## 4. Cache-stable prompt layout

Fixed order: (1) contract system snippet, (2) tool schemas, (3) boot context, (4) conversation.
Layers 1–3 never change mid-session — not on a mode change either (mode is announced in the
conversation). MCP and deferred tools never enter the tool list; `tool_search` returns schemas as
tool-result text and the model calls `mcp_call`. Boot context lists skills as `name — description`
(bodies are read with `read`), deferred tools by name, a short repo map, repo instructions and a
bounded set of the operator's remembered preferences/corrections, within 2,000 tokens.

Cache-breaking operations — model switch, MCP server add/remove, skill install, contract or
AGENTS.md change — happen only on explicit operator command, log `cache_break` with a reason,
and print the estimated cost of re-paying the prefix. Anthropic requests place
`cache_control` breakpoints after the tools block, after the boot context, and on the last
user turn. OpenAI-kind requests send `prompt_cache_key = session_id` when the provider accepts
it (OpenAI and xAI do; unknown endpoints ignore unknown fields).

## 5. Sandbox

One implementation: `zsandbox` from `rust-tui/crates/zsandbox`, used by all three
harnesses:

```
zsandbox exec --workspace <dir> --net deny|allow --timeout <s> -- <argv...>
```

Windows: launch inside an AppContainer with no capabilities (denies network by default);
grant the AppContainer SID read/write on the workspace and read/execute on toolchain dirs;
wrap the process in a Job object with `KILL_ON_JOB_CLOSE`, a memory cap and a process cap.
If AppContainer setup fails, fall back to a restricted token at low integrity inside the Job
object and report `sandbox:"degraded", reason` in `session_start` and every `bash` result.
Linux: Landlock + seccomp or bubblewrap (Codex lineage). Escalation only via
`request_escalation`; never silent; denied in headless unless `--allow-escalation network`.
Never change firewall, Defender, or system policy.

## 6. Event log

Sessions are append-only JSONL at `COSMOS_CLI_HOME\sessions\<session_id>.jsonl`:

```json
{"v":1,"id":"<ulid>","parent":"<ulid|null>","ts":"<rfc3339>","session":"<ulid>","harness":"cosmostui|zdev|zagy","kind":"...","data":{...}}
```

Kinds: `session_start` (model, mode, sandbox, boot_tokens), `session_end`, `checkout`,
`user`, `assistant`, `tool_call`, `tool_result`, `plan`, `permission_request`,
`permission_decision`, `usage` (`{input, cache_read, cache_write, output, usd, cum_usd}`),
`model_switch`, `cache_break`, `compaction`, `subagent_start`, `subagent_end`, `handoff`,
`error`, `mode`, `snip`, `compact`, `compact_thrash`, `tool_repair`, `checkpoint`,
`verify`, `self_improve`, `job_start`, `job_end`, `doom_loop`, `aside`.

Branching: `session branch <session> --at <event_id>` appends a `checkout` event; replay
follows `parent` from the current leaf. `session tree` prints the tree. Every harness supports
`session list` and `session export --format md|jsonl` over any harness's logs, and
`--resume <id>` for its own.

## 7. Headless and exit codes

Flags: `-p "<prompt>"` (`-` = stdin), `--output text|json|json-stream`, `--mode plan|agent`,
`--allow <tools,...>`, `--yolo`, `--model <alias>`, `--max-turns N`, `--max-usd X`,
`--cwd <dir>`, `--resume <id>`, `--sandbox …`, `--ask …`, `--autonomous`,
`--autonomous-gate "<cmd>"`, `--allow-self-mutate`, `--arena 1`. `json-stream` writes
exactly §6 events, one per line, nothing else. Headless `/compact`, `/undo`, `/skillify`
short-circuit without a model turn.

Exit codes: 0 done · 1 task failed · 2 usage error · 3 permission denied · 4 provider/network
error · 5 budget exceeded.

## 8. Cost ledger

After every model turn print to stderr (headless) / status bar (TUI):

`turn 3 · in 1,204 · cache_r 18,400 · cache_w 0 · out 312 · $0.0041 · Σ $0.0213 · ctx 41%`

Provider usage mapping: OpenAI-kind `usage.prompt_tokens_details.cached_tokens` → cache_read
(remaining prompt_tokens → input; `completion_tokens` → output); Anthropic
`cache_read_input_tokens`/`cache_creation_input_tokens`/`input_tokens`/`output_tokens`;
Gemini `usageMetadata.cachedContentTokenCount`/`promptTokenCount`/`candidatesTokenCount`.
Unknown price prints `$?` — never a guess.

## 9. Compaction policy (cache-aware)

Compact only when: (a) projected next-turn input > 0.80 × context window, or (b) the provider
cache is presumed cold (idle longer than the provider TTL — Anthropic default 300 s, others
600 s) AND context > 0.50 × window. Never compact while the cache is warm and context < 80%.
Compaction replaces conversation only — layers 1–3 are untouched. Logs
`compaction {before_tokens, after_tokens, reason}`. Five layers, cheapest first: L1 when a result
is produced, anything over 8,000 chars (24,000 for `read`) is persisted under
`cache/tool/<sha>.txt` and replaced, in the log and the model view alike, by a 2,000-char
middle-out preview with `log_path` and `omitted_chars`; L2 snip after 48 messages (keep 3 head +
40 newest); L3 stub tool results older than 6 turns as `{tool, ok, path, sha}`; L4 stub a `read`
superseded by a later read of the same path and range; L5 one summary call (goal, decisions,
files, failing commands, next step) → `COSMOS_CLI_HOME/world/<session>.md`, keeping the newest
~20,000 tokens verbatim. L2–L5 are **sticky**: each is decided once at a checkpoint, logged as
`snip`/`compact`/`compaction` with a `view` op, and replayed identically on every later request and
on resume, so between checkpoints each request extends the previous one and the cache keeps
hitting. The raw JSONL is never rewritten. Cache-warm = last model call within the provider TTL
(Anthropic 300 s, others 600 s) AND prefix hash unchanged. `/compact focus: …` forces L5.
Thrash (still over the threshold on the turn right after an L5) logs `compact_thrash` and
stops auto-L5 until the operator runs `/compact`.

## 10. Subagents

`task` runs a fresh conversation over the same stable prefix (cache shared); returns only
final text capped at 4,000 chars; child events go to a child session file linked by
`subagent_start.data.child_session`. `worktree:true` runs
`git worktree add COSMOS_CLI_HOME\worktrees\<session>\<task_id>` and returns the branch name;
removal happens on `session_end` only if the worktree is clean. Parallel tasks bounded by
`max_parallel`.

## 11. Skills, hooks, AGENTS.md, MCP, ACP

- Skills: discovered at `<repo>\.agents\skills\*\SKILL.md` and
  `COSMOS_CLI_HOME\skills\*\SKILL.md`; boot context carries `name — description — path` only;
  the body is read through `read`.
- Hooks: events `session_start`, `user_prompt`, `pre_tool`, `post_tool`, `pre_compact`,
  `stop`, `subagent_stop`, `session_end`. The command gets the event JSON on stdin. Exit 0
  continues; exit 2 blocks and feeds stderr back as the tool result; any other code logs
  `error` and continues. Built-in `pre_tool` hook `large_read_guard`: a `read` with no range
  on a file longer than `large_read_lines` is blocked and the bridge `cache.read` summary is
  returned instead; explicit ranges ≤200 lines, or `force_raw` after a grant, bypass it.
- AGENTS.md: repo-root + nearest-to-cwd copies count against the boot budget; overflow is
  truncated with a `warning` event telling the operator to move content to Meridian playbooks.
- MCP: stdio client speaking `initialize`, `tools/list`, `tools/call`; the server set is fixed
  at session start.
- ACP: `<bin> acp` serves `initialize`, `session/new`, `session/prompt`, `session/update`
  (notifications), `session/request_permission`, `session/cancel` over stdio JSON-RPC.

## 12. Privacy

Outbound hosts = configured provider `base_url`s + configured MCP endpoints. No telemetry, no
crash upload, no git-history upload. Every HTTP client goes through a transport wrapper that
rejects any host not on the allowlist; a test proves the rejection.

## 13. Repair, checkpoints, self-improve

- Tool-call repair (in order): unwrap fences → alias remap (`Read`→`read`) → drop unknown
  keys → coerce types → reject missing required. Logs `tool_repair`. Same hash+args three
  times with no file change is `doom_loop` and the turn stops.
- Checkpoints: every `write`/`edit` copies the prior file to
  `COSMOS_CLI_HOME/ckpt/<session>/<n>-<basename>`. `/undo` restores the last one (not git).
- Self-improve is bounded: skills, preferences, corrections, playbooks. Default on for
  interactive text, off for CI/`json-stream`. The agent may not rewrite its binary,
  `CONTRACT.md`, or sandbox policy without `--allow-self-mutate` plus a worktree plus a
  green conformance gate. `/skillify <name>` writes `skills/<name>/SKILL.md`. `/refine`
  goes through Meridian `refine.propose|apply|rollback`.
- `task.write` defaults `worktree` to true. Subagent returns ≤4000 chars; parent never
  ingests child tool traces.
- Read-only tools may run in parallel (cap 8). Writes, bash, mcp, and `task` stay serial.
- GPUI: `zgpui` is a typed ACP client crate. Daily `cosmostui` does not link GPUI.
