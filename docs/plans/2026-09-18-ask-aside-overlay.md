# Delegate a task — global "Cosmos anywhere" overlay plan

> A Spotlight/Raycast-style bar summoned from anywhere in the OS by a global
> hotkey (Alt+Space default; Cmd+Space and any rebindable combo supported).
> Type a task, attach screen context, pick a model, send — Cosmos mints a
> real session and runs it, no main-window trip required.
> Reference: the floating-bar design screenshot — same interaction shape,
> but rendered in Cosmos's own UI/UX: theme palette, Geist/Geist Mono,
> hairline borders, `popover_card` surface, `menu-in` entrance — and the
> prompt is "Delegate a task to Cosmos", not "Ask Aside".

## What already exists (reuse, don't rebuild)

- **Global-hotkey plumbing**: `appshots::shortcut` (combo parse/validate,
  live re-registration, subscribers), Carbon `RegisterEventHotKey` backend
  (`appshots/macos.rs`), GlobalShortcuts-portal + X11 grab backends
  (`appshots/linux/`). Windows has **no** global-hotkey backend today —
  `appshots` is `macos|linux` only (`is_desktop()`).
- **Popup window kind**: `WindowKind::PopUp` in the pinned zui rev.
  macOS → `NSPanel` at `NSPopUpWindowLevel`, `NonactivatingPanel`,
  `CanJoinAllSpaces|FullScreenAuxiliary` — floats over other apps on every
  space. Windows → `WS_EX_TOOLWINDOW` borderless tool window (no taskbar
  entry) — but **not topmost**; we must `SetWindowPos(HWND_TOPMOST)` on the
  HWND ourselves (`HasWindowHandle` pattern already used in
  `browser/windows.rs`) or patch zui. Rounded corners via
  `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE)` on the same HWND.
- **The bar's internals**: `Composer` already owns send/queue/attachments/
  appshots/model+reasoning pickers/drafts, and the canvas `send()` path
  mints a real session on first send (`Mutate createChat` + Run command
  with the picked `ChatConfig`, space, cwd, worktree plan). `new_for_chat`
  (side-chat binding) proves the entity runs outside the shell column.
- **A session for the task**: delegation = the composer's `is_new` send —
  a real (non-archived) chat on the resolved space/device so it shows up
  in the sidebar like any session.
- **Delivery + focus**: `deliver_appshot` finds/reopens the Shell window;
  `open_notified_chat` routes notification clicks to a chat;
  `start_appshot_service` shows the spawn-a-task-on-hotkey +
  press-coalescing pattern.
- **Settings**: `ShortcutId` + `KeymapConfig` + the Settings → Shortcuts
  table give a rebindable row with conflict detection nearly free.
- **Styling**: `Theme::of(cx)` semantic palette, `typography` Geist fonts,
  `popover::popover_card`/`menu_row`, `motion::menu-in`, wash hover blends,
  frost `window_background` — the bar should read as a detached piece of
  the app's own chrome, not a foreign widget.

## Architecture

### 1. `crates/ui/src/global_hotkey/` — extract the shared registry

Pull `appshots::shortcut` up into a feature-neutral module: `Shortcut`
(parse/validate), a `GlobalHotkeyId { CaptureAppshot, DelegateTask }` key,
and `register(id) -> UnboundedReceiver<()>` per platform.

- **macOS**: second Carbon hotkey next to the appshot one — distinct
  `EventHotKeyId` signature/id, same installed handler dispatching by id.
- **Windows** (new): dedicated thread owning a message-only HWND;
  `RegisterHotKey(MOD_NOREPEAT)` per id; `WM_HOTKEY` → mpsc sender;
  unregister/re-register on `set_shortcut`. VK map for the `Shortcut::key`
  vocabulary (space, tab, enter, alnum, nav keys, F1–F20). Registration
  failure (e.g. PowerToys owns Alt+Space) → capability `Unavailable`
  surfaced in Settings, same shape as `AppshotCapabilities.global_shortcut`.
- **Linux**: Wayland binds a second portal shortcut id (`delegate-task`);
  X11 takes a second passive grab on the existing connection.
- `appshots` keeps only capture logic and registers `CaptureAppshot`
  through the shared module.

### 2. `crates/ui/src/delegate/` — the overlay surface

- `mod.rs`: enable flag, `open/toggle/close` entry points, hotkey-event
  loop (mirror `start_appshot_service`: coalesce queued presses, ignore
  while a send is in flight).
- `window.rs`: `cx.open_window` with `kind: WindowKind::PopUp`,
  `titlebar: None`, `is_resizable: false`, `is_minimizable: false`,
  `is_movable: true`, theme-resolved `window_background` (translucent
  frost where supported); bounds = centered horizontally, ~30% from the
  top of the display hosting the cursor (`display_id` in `WindowOptions`).
  Windows: post-open HWND pass — `SetWindowPos(HWND_TOPMOST)` + DWM
  rounded corners; snapshot `GetForegroundWindow()` *before* opening
  (screen-context target + focus restore on dismiss). macOS: verify the
  nonactivating panel accepts keyboard input (Spotlight-style panels do);
  fallback is `WindowKind::Floating` + manual level. Linux: `PopUp` now,
  `WindowKind::LayerShell` as the Wayland-native upgrade path.
- `delegate.rs` (the view): one Cosmos-styled card (`popover_card`
  treatment) hosting a **delegate-mode Composer** — a `new_delegate`
  constructor that pins `current_key` to a dedicated `"delegate"` draft
  and forces `target_chat_id → None`, so `send()` always takes the
  `is_new` path (mints a real session with pickers' space/device/config).
  Rendered as the compact row: input ("Delegate a task to Cosmos…"
  placeholder), `+`, screen-context, model picker, send. Pickers give the
  model/reasoning popover for free; project/space picks resolve from the
  same defaults the new-session canvas uses (sidebar filter → last space).
- Dismissal: hotkey toggles; Esc closes; click-outside/blur closes;
  `window.remove_window()` (close-and-reopen per summon — no hide/show
  bookkeeping). Successful send closes the bar and leaves a status toast
  in the shell; optionally keep the bar open with a thin "Delegated ✓ /
  Working…" pill — decide in review.

### 3. The delegated session

- No persistent hidden chat: each delegation mints a real session through
  the existing canvas send path (`Mutate createChat` with `ChatConfig` +
  cwd + space, then the Run command). It appears in the sidebar normally
  and notifies on completion/input-needed via the existing `notify` →
  `open_notified_chat` path.
- "Open session" affordance (from the bar's post-send state or the toast):
  `shell.open_chat(chat_id)` + `window.activate_window()`.
- The delegate draft (text + staged attachments/appshots under
  `current_key = "delegate"`) survives bar dismissal — reopening the bar
  restores the half-written task.

### 4. Screen-context button

- Capture the app that was frontmost **before** the bar took focus
  (recorded at hotkey time — the bar itself trips the `active_window`
  self-capture guard otherwise). Stage via
  `composer.stage_appshot_for("delegate", …)`.
- macOS: `appshots::capture_active_window()` works as-is — the panel never
  activates, so the user's app stays frontmost.
- Linux: same path (portal/X11 capture).
- Windows: needs the missing appshot Windows backend — capture the
  recorded pre-popup foreground HWND via Windows Graphics Capture
  (screenshot first; UIA semantic text as follow-up). Hide the button on
  Windows until that lands. (Shared win: unblocks Windows Appshots too —
  track separately.)

### 5. Settings + CLI

- `ShortcutId::DelegateTask` — label "Delegate a task", default
  `alt-space` everywhere; `KeymapConfig::delegate_task`; `available()` =
  all desktop platforms (unlike `CaptureAppshot`). Cmd+Space works on
  macOS via Carbon (it can preempt Spotlight) — user's choice, not the
  default.
- `delegate_enabled` toggle in `UiSettings` (mirrors `appshots_enabled`).
- `cosmos delegate` subcommand → activation socket → running headed
  instance, same as `zeron appshot` on Linux — covers Wayland desktops
  with no portal.

## Guardrails

- Self-capture: the bar IS a viewer window, so the existing
  `active_window().is_some()` guard would always trip — screen context
  must capture the recorded pre-popup foreground window, not "whatever is
  frontmost now".
- Hotkey conflicts → `Unavailable` state in Settings, never silent.
- Delegating must not secretly target the shell's selected chat — the
  composer needs the explicit always-new mode (`target_chat` override is
  not enough: `None` falls back to `state.selected_chat`).
- The feature only works while Cosmos runs; tray/launch-at-login is a
  stated follow-up (gpui has no tray today).
- Offline/queueing: the canvas send path already rides the durable
  command queue — a delegated task while offline queues like any send.

## Phasing

1. `global_hotkey` extraction + Windows `RegisterHotKey` backend +
   `ShortcutId::DelegateTask` + settings row + enable flag.
2. Popup window + delegate view + always-new composer mode + send path
   (Windows + macOS; Linux PopUp/portal as it lands).
3. Screen-context button (macOS/Linux day one; Windows behind the capture
   backend).
4. Polish: `menu-in` entrance, capture sound reuse, delegated status
   pill/toast, Esc/blur dismiss tuning, `cosmos delegate` CLI.

## Verification

- `cargo test -p zeron-ui --lib`: shortcut parse/portal-spell tables (new
  VK map tests), delegate always-new send resolution (regression:
  `target_chat_id` must not fall back to `selected_chat`), dismiss/toggle
  state machine, `active_window` guard test (pattern in
  `appshot_activation_tests`).
- `cargo check -p zeron-ui` + `cargo check -p zeron` on Windows, macOS,
  Linux; `cargo fmt --all -- --check`.
- Native matrix: Win10 1903+/11 (hotkey conflict case incl. PowerToys
  Run), macOS (fullscreen spaces, nonactivating focus), GNOME/KDE Wayland
  (portal v2/v3) and X11.
