# t03 — tool-call repair (Read → read)

Fixture-driven. Open-model alias `Read` must be repaired locally to `read`.

## Setup

`workspace/a.txt` contains `hello`.

## Steps

1. `tool:Read {"path":"a.txt"}` — harness remaps `Read`→`read`.
2. Stream contains a `tool_repair` event with `reason=alias`.
3. `tool_result` is a successful hashline read (not `unknown tool`).

## Pass criteria

- exit 0
- `tool_repair` present
- tool result contains `file_sha256=` and `hello`
