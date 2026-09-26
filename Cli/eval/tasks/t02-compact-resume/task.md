# t02 — compact resume keeps the goal

Fixture-driven. A session is compacted, then resumed. The agent must still
know the original goal from `world.md` without the operator repeating it.

## Steps

1. Headless `-p "ship the login form"` with fixture (creates a session + world goal).
2. Headless `-p /compact --resume <id>` — writes `COSMOS_CLI_HOME/world/<id>.md`.
3. Resume again with `-p ping` — session still schema-valid; world.md still contains the goal.

## Pass criteria

- `/compact` exits 0
- `world/<id>.md` exists and contains `ship the login form`
- resumed events validate
