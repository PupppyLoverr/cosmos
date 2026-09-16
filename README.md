# Zeron

Control your coding agents (Claude Code, Codex, Cursor, Devin, Google Antigravity, Amp, OpenCode, Grok, Hermes, Pi) locally by default, with optional multi-device sync.

*English | [简体中文](README.zh-CN.md)*

![Zeron driving a Claude Code session with a live branch diff sidebar](apps/landing/public/assets/app-screenshot.jpg)

Every device runs a small engine that stores sessions on that device. A new installation starts in local-only mode without an account or a network connection.

## Install and run locally (Linux)

```bash
curl -fsSL https://zeron.sh/install.sh | sh
zeron status
```

The installer starts the daemon immediately and keeps it running across reboots. No sign-in or sync configuration is required.

The desktop sidebar browser also needs the [Linux browser runtime](docs/reference/linux-browser.md).

## Install on Windows

Download `Zeron-Setup-x64.exe` (per-user installer, no admin) or the portable `zeron-windows-x86_64.zip` from the latest release. The zip also contains `install.cmd`, which drops `zeron.exe` into `%LOCALAPPDATA%\Zeron\bin` and adds it to your user `PATH` — new terminals then resolve `zeron`.

```powershell
zeron.exe            # desktop app
zeron.exe headless   # engine only
zeron daemon install # per-user Task Scheduler entry (runs at logon)
zeron daemon start|stop|restart|status
```

The Windows terminal uses ConPTY and defaults to `cmd`/`powershell` (override with `COMSPEC`); Git for Windows is used for workspace operations. No WSL is required for Zeron itself.

Day-to-day:

```bash
zeron status      # local/synced mode and engine status
zeron update      # update to the latest release
zeron daemon start|stop|restart|status
```

## Harnesses and models

Zeron detects installed agent CLIs itself: `PATH` (including `.exe`/`.cmd` npm shims on Windows), your login shell's `PATH`, the Node version-manager bins (fnm/nvm/volta/pnpm/bun), and each harness's known install dirs. An undetected harness shows as *not installed* in Settings → Harnesses rather than failing; an `*_EXECUTABLE` env var (e.g. `CLAUDE_CODE_EXECUTABLE`, `OPENCODE_EXECUTABLE`) pins a specific binary.

The model picker queries each harness's own catalog — `agy models` for Antigravity, the configured providers from the running OpenCode server, ACP config options for Devin/Grok/Hermes/Pi, Amp's Dial modes (`low`/`medium`/`high`/`ultra`) — so newly released models appear without a Zeron update. Catalogs refresh in the background at startup and on picker open; a failed refresh keeps the last-known-good list. Selections persist per workspace and are passed through to the real launch configuration (`--model`, `--effort`, mode flags).

## Optional multi-device sync

Sign in only when you want to open your account's synced workspace. Authentication changes the profile selected by the next engine start, so stop the daemon before changing it:

```bash
zeron daemon stop
zeron login
zeron daemon start
```

You can then start an agent on one synced device and follow or drive it from another. An always-on machine such as a VPS can keep those agents working after you close your laptop.

Devices signed in to the same synced account are trusted with remote workspace access. A device controlling a workspace on another device can list, read, and write its files; enabling `Show ignored files` also makes gitignored files such as `.env` available remotely. `.git` is always excluded. Only sign in devices you trust with the full contents of your workspaces.

Signing in does not upload, move, or import existing local sessions. Local sessions and their attachments remain under the local profile and reappear when you return to local-only mode:

```bash
zeron daemon stop
zeron logout
zeron daemon start
```

`zeron login` and `zeron logout` refuse to modify credentials while an engine owns the data directory. The desktop app follows the same next-restart profile boundary.

On macOS: use the desktop release, or build `zeron` from source and run `zeron daemon install` to install the launchd service.

---

Developing or curious how it works? [![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/zeronsh/zeron) or check out [ARCHITECTURE.md](ARCHITECTURE.md).

Licensed under the [MIT License](LICENSE).
