# Cosmos (zeron) — Folder Overview

Snapshot: `main` at PR #43 (`b6acb972`), workspace version 0.2.91.

## What it is

Cosmos is a native desktop app and background engine for controlling coding agents: Claude Code, Codex, Cursor, Devin, Google Antigravity, Amp, OpenCode, Grok, Hermes and Pi. It is a ground-up Rust rewrite of the earlier TypeScript/Electron "zeron" product, with a [gpui](https://www.gpui.rs/) UI (the framework behind Zed). It is MIT-licensed.

Key ideas:
- **Local-first.** A fresh install runs in local-only mode with no account or network. Sessions are stored on the device.
- **Optional multi-device sync.** After sign-in (WorkOS), a session started on one device can be followed or driven from another. An always-on VPS can keep agents running while the laptop is closed.
- **One binary, headed or headless.** `cosmos` opens the UI. `cosmos headless` runs the engine only. If a daemon is already listening, the UI attaches to it; otherwise the engine runs in-process and also serves the IPC port.
- **Cloud computers.** Recent work adds cloud runners (Railway), snapshot and continuity of sessions, and remote transcript streaming.

## Architecture in brief (see `ARCHITECTURE.md`)

```
gpui UI ─ RPC ─ engine A ══ DeviceRoom DO relay ══ engine B ─ RPC ─ gpui UI
                    └─ optional chat2 sync ── ChatRoom DO (per chat)
```

- **Engine** runs agents, owns auth, terminals, repos/worktrees, diff sync and doc hosting. It is fully functional headless.
- **UI** is a viewport onto engine state, organised around *spaces* (device + folder pairs). The sidebar is an attention-sorted session list, and tabs are a device-local view.
- **Edge** is a TypeScript Cloudflare Worker with Durable Objects: a `ChatRoom` per chat, a `DeviceRoom` per device, R2 attachments and WorkOS JWKS auth. Session docs are Loro CRDTs relayed as opaque append-only rows. The workspace registry is a replicated row table (per-field LWW/HLC), not a CRDT.
- **Workspace profiles.** `AuthState` (live credentials) is separate from `WorkspaceScope` (`Local`, `Synced` or `Development`), which is fixed at engine start. This keeps a login or token refresh from swapping the open store. Each scope has its own store, journals and uploads directory.

## Directory map

| Path | Purpose |
| --- | --- |
| `crates/` | Rust workspace, 18 crates (below) |
| `apps/zeron` | Main desktop/headless binary, shipped as `cosmos` with a `zeron` alias |
| `apps/android`, `apps/ios`, `apps/mobile` | Mobile clients |
| `apps/landing`, `apps/www-redirect` | Marketing site and redirect |
| `Cli/` | Side tooling in other languages: `rust-tui` (the `cosmostui` CLI), `go-ssh`, `python-cloud`, `meridian-bridge`, `zdev`, `contracts`, `eval`, `.research` |
| `edge/` | TypeScript Cloudflare Worker and Durable Objects (`zeron-edge`; vitest, wrangler, `jose`, `loro-crdt`) |
| `docs/` | ADRs, plans, research, performance, regressions, references, cloud-continuity and cloud-session-store notes |
| `scripts/` | Build and release scripts, iOS helpers, `install-runner.sh` |
| `.github/`, `.cargo/`, `.agents/` | CI, cargo config, agent config |
| `dist/`, `release-artifacts/`, `target/` | Build outputs |

### Crates

- **Core:** `engine`, `harness` (agent detection and launch), `proto`, `rpc`, `sync`, `client`, `mcp`, `update`, `tools`, `computer`
- **UI and rendering:** `ui`, `theme`, `text`, `syntax`, `markdown`, `doc`, `preview`
- **Mobile:** `mobile`

## Root files

- `README.md` / `README.zh-CN.md` cover install and usage.
- `ARCHITECTURE.md` is the design reference.
- `CONTEXT.md` is a domain glossary, mostly theme vocabulary (theme family, variant, linked theme, accent override, surface treatment).
- `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` define the workspace (edition 2024).
- `THIRD_PARTY_NOTICES.md`, `LICENSE`, `CONTRIBUTORS.md` and `skills-lock.json` are the remaining metadata.

## Install and usage

- **Linux:** `curl -fsSL https://zeron.sh/install.sh | sh`, then `zeron status`. The installer starts a persistent daemon.
- **Windows:** `Cosmos-Setup-x64.exe` (per-user) or the portable zip, which provides `cosmos.exe` and a `zeron.exe` alias. The terminal uses ConPTY and needs no WSL.
- **Commands:** `cosmos status`, `cosmos update`, `cosmos daemon start|stop|restart|status`, `cosmos login` and `cosmos logout` (run these with the daemon stopped).
- **Harness detection:** searches `PATH`, the login shell, Node version-manager bins and known install dirs. Override with `*_EXECUTABLE` env vars. Model catalogs are queried live from each harness.

## Git state

- `origin` is `PupppyLoverr/cosmos`, a fork. `upstream` is `zeronsh/zeron`.
- Recent history is largely Devin-authored PRs (#29–#43) covering installers, cloud selection persistence, Railway trial signup and reachability, cloud stream rendering, cloud continuity, and remote transcript folding.

## Housekeeping notes

- Stray debug captures sit at the root (`cursor-acp-*`, `grok-*`, `hermes-*`, `cursor-session.json`), along with `.zsandbox-tmp/`. They are probably not meant to be committed.
- `Notes/` is untracked (it contains `koala-facts.md`).
- Naming is mid-transition between zeron, Cosmos and cosmostui. The README installer URL still uses `zeron.sh`.
