# t01 — read → write → grep → verify

Fixture-driven eval task. Runs against the `fixture` provider using `tool:`
prompts, so it is deterministic and works with no live model. Each step is a
fresh `-p` invocation sharing the same workspace.

## Setup

`workspace/` contains `src/app.py` (prints `hello v1`).

## Steps the harness must prove

1. `tool:read {"path":"src/app.py"}` — emits `file_sha256=` + anchored lines.
2. `tool:write {"path":"notes.md","content":"done"}` — new file appears.
3. `tool:write` rewrite `src/app.py` to `print("hello v2")` — file shows v2.
4. `tool:grep {"pattern":"v2"}` — finds the edited line.
5. Every session file validates against `eval/event.schema.json`.

## Pass criteria

- every invocation exits 0
- `src/app.py` contains `v2`; `notes.md` exists
- grep tool_result contains `v2`
- all session events schema-valid
