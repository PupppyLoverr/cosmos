---
name: cosmos-computer
description: Drive the desktop through Cosmos Computer MCP tools (computer_*), never raw cua-driver, xdotool, wmctrl, or AT-SPI scripts.
---

# Cosmos Computer

You are driving the user's desktop through **Cosmos Computer MCP**.

The path is: OpenCode → Cosmos Computer MCP → Cosmos backend → Cua Driver → OS.

## Allowed

Call these MCP tools (prefix may be `cosmos-computer_` / `computer_*`):

- `computer_status` first
- `computer_list_apps` / `computer_list_windows`
- `computer_pin_window` once you pick a target (required when other agents may run)
- `computer_observe` / `computer_find` (semantic first; `includeScreenshot` only when needed; pass `maxElements` ≤ 400 and `windowId` from `list_windows`. If you get an X11 fallback observe, continue with pin + hotkey — do not wait on a hung AT-SPI tree.)
- `computer_click` / `computer_double_click` / `computer_right_click`
- `computer_type_text` / `computer_set_value` / `computer_press_key` / `computer_hotkey` (`keys` MUST be an array e.g. `["ctrl","s"]` or `["escape"]`, never the string `"ctrl+s"`)
- `computer_scroll` / `computer_scroll_until`
- `computer_invoke_menu`
- `computer_wait` / `computer_verify`
- `computer_clipboard_read` / `computer_clipboard_write`
- `computer_launch_app` / `computer_focus_app` / `computer_set_window_frame`
- `computer_stop` when the user says STOP

## Forbidden for GUI actions

Do **not** use the shell for GUI automation:

- `cua-driver` / `cua-driver call` / `cua-driver mcp`
- `xdotool`, `wmctrl`, `ydotool`, `wtype`
- AT-SPI Python (`gi.repository.Atspi`), `pyatspi`
- clicking via screenshots + coordinate guessing

Shell is fine for reading/writing files, `ls`, `cat`, compiling — not for clicking windows.

## Canonical loop

1. `computer_status` — abort if disabled / observe-only when you need to act.
2. `computer_list_windows` — pick **pid + windowId**, never title alone.
3. `computer_pin_window` for this session.
4. `computer_observe` (or `computer_find`) — record `snapshotId` and E-refs.
5. Act with `elementToken` or `elementIndex` + `snapshotId` + `pid` + `windowId`.
6. After **any** action or UI change: re-observe. Refs are snapshot-bound.
7. `computer_verify` or `computer_wait` instead of sleeping.
8. On `stale_element` / `stale_snapshot`: re-observe, never retry the same ref.
9. On `background_unavailable`: retry once with `deliveryMode: "foreground"` only if status shows Full Control. Do not start with foreground.
10. Transient UI (file picker, dialog, context menu): `computer_wait` then fresh observe.

## Linux notes (X11)

- `computer_list_windows` and semantic-less `computer_observe` use the X11 client list (milliseconds). Pass `windowId`.
- `computer_hotkey.keys` MUST be an array (`["ctrl","s"]`, `["escape"]`), never the string `"ctrl+s"`.
- When AT-SPI / Cua stdio hangs, Cosmos still performs type/hotkey/click/focus/clipboard through its X11 input path. You still only call `computer_*`.
- Background delivery via AT-SPI is often unavailable; if you get `background_unavailable`, retry once with `deliveryMode: "foreground"` under Full Control.
- Wayland (if `computer_status.session` is `wayland`): portal capture, no unique window targeting, background often refused. Do not fake Wayland support.
- GTK apps may need `GTK_A11Y=1` for a real accessibility tree.
- The Cosmos GPUI window itself is omitted from `list_windows` (AT-SPI walks hang on it).

## Isolation

If you are one of several parallel agents, pin immediately and never click another agent's pid. `window_isolation` means you targeted the wrong window — re-observe your pin.

## STOP

Call `computer_stop` and halt. Do not keep clicking.
