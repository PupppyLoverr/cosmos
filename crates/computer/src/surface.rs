//! The `computer_*` MCP tool surface — schemas plus the dispatch shared by
//! the loopback endpoint (harnesses) and the engine RPC (UI/tests). One
//! funnel means the control-mode policy lives in exactly one place.
//!
//! Description contract — every schema teaches the same grammar:
//! - `E{n}` refs come from `computer_observe`/`computer_find` digests and are
//!   valid ONLY for the snapshot that produced them (`snapshotId`). Re-observe
//!   after UI changes; a stale ref fails closed with `stale_element`.
//! - `pid`+`windowId` identify the exact window (from `computer_list_windows`
//!   or an observe header). `windowId` defaults to the pid's frontmost.
//! - `deliveryMode` is `background` by default — never steals focus. Only
//!   retry with `foreground` after an explicit `background_unavailable` error
//!   AND only when `computer_status` shows Full Control on.
//! - x/y are WINDOW-LOCAL pixels matching the observed screenshot space.

use serde_json::{Value, json};

/// Every tool the Cosmos Computer MCP server advertises. Keep this list the
/// ONLY place tool names live — `tools/list` and `tools/call` both read it.
pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "computer_status",
            "description": "Report Cosmos Computer posture on this device: enabled flag, control mode (observe|background|full-control), driver install+health, semantic/screenshot capability, OS permission hints, last error. Call this FIRST — a disabled or observe-only computer turns every action into a structured refusal.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        }),
        json!({
            "name": "computer_observe",
            "description": "See a window or the desktop. scope 'frontmost' (default) describes the frontmost non-Cosmos window; optional pid plus windowId/window_id pins any scope to an explicit target (windowId preserves exact identity, while pid alone picks that app's top visible window); 'window' requires pid; 'desktop' lists every on-screen window grouped by app. Returns a COMPACT digest: one line per meaningful element with E{n} refs, roles, labels, values, states and window-local bounds — not a raw accessibility dump. E-refs are valid only against the returned snapshotId; re-observe after UI changes. includeScreenshot adds a window PNG as an image block (requires user-enabled screenshots).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scope": {"type": "string", "enum": ["frontmost", "desktop", "window"], "description": "What to observe. Default 'frontmost'."},
                    "pid": {"type": "integer", "description": "Optional explicit target process id from computer_list_windows. With frontmost/default scope, pins observation to this app; with no windowId, selects its top visible window."},
                    "windowId": {"type": "integer", "description": "Optional exact target window handle from computer_list_windows. Supplying it with pid preserves that exact window even when another sibling is frontmost. Alias window_id is also accepted."},
                    "window_id": {"type": "integer", "description": "Snake-case alias for windowId."},
                    "includeScreenshot": {"type": "boolean", "description": "Also return the window screenshot as an image (only when the user enabled screenshots in Settings → Computer)."},
                    "format": {"type": "string", "enum": ["digest", "full"], "description": "'digest' (default) = compact element lines. 'full' = the driver's raw accessibility tree — big; use only when the digest hides what you need."},
                    "maxElements": {"type": "integer", "description": "Accessibility walk cap. Defaults to the user's setting."},
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_find",
            "description": "Search a window's FULL element set by text — 'Save', 'Bluetooth', 'Repository name' — optionally narrowed by role (button|edit|menuitem|checkbox|tab|...). Returns a ranked set of E-refs with labels/values/bounds, capped small. Use this instead of re-observing when the digest hid or truncated what you need, or when you want the exact ref for a known control.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Target process id."},
                    "windowId": {"type": "integer", "description": "Target window handle (defaults to the pid's frontmost)."},
                    "query": {"type": "string", "description": "Text to match against element labels and values (word-insensitive contains; exact matches rank first)."},
                    "role": {"type": "string", "description": "Optional role filter, e.g. 'button', 'edit', 'menuitem', 'checkbox'."},
                    "maxResults": {"type": "integer", "description": "Max hits (default 8, cap 20)."},
                },
                "required": ["pid", "query"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_wait",
            "description": "Wait for a UI condition instead of fixed sleeps — the right call after launch_app, clicks that open dialogs/menus, and loads. 'for' picks ONE condition: {element:{role?,labelContains}} appears, {elementGone:{...}} disappears, {text:'str'} appears anywhere, {window:true} exists for the pid, {windowGone:true} is gone, {stable:true} two consecutive identical snapshots (UI settled). Polls adaptively (100–300ms) up to timeoutMs (default 10s, cap 45s). Returns the satisfied state or a timeout error with the last observed summary.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Target process id."},
                    "windowId": {"type": "integer", "description": "Target window handle (defaults to the pid's frontmost)."},
                    "for": {"type": "object", "description": "One condition: {element:{role?,labelContains}} | {elementGone:{...}} | {text:'str'} | {window:true} | {windowGone:true} | {stable:true}."},
                    "timeoutMs": {"type": "integer", "description": "Wait cap in ms (default 10000, max 45000)."},
                },
                "required": ["pid", "for"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_verify",
            "description": "Deterministically check a postcondition on ONE window — the 'did it work' step after an action. 'expect' is an array of predicates ANDed: {element:{selector:{role?,labelContains}, exists?:true, enabled?, selected?, valueEquals?}} or {window:{exists?,bounds?}}. Each returns satisfied|unsatisfied|unknown — 'unknown' is NOT success (ambiguous/unprovable). Prefer re-observing for free-form checks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Target process id."},
                    "windowId": {"type": "integer", "description": "Target window handle (defaults to the pid's frontmost)."},
                    "expect": {"type": "array", "description": "1-8 predicates, e.g. [{\"element\":{\"selector\":{\"labelContains\":\"Save\"},\"exists\":true,\"enabled\":true}}]."},
                },
                "required": ["pid", "expect"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_list_apps",
            "description": "List applications — running and installed — with name, pid (0 when not running), active flag and launch path. Resolve a user-visible app name to a pid here, then use computer_list_windows/observe for windows.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
        }),
        json!({
            "name": "computer_list_windows",
            "description": "List top-level windows with pid, app name, title, bounds, z-order and minimized flags — the window-targeting inventory. Frontmost = highest z. Two windows can share a title: target by pid+windowId, never by title alone.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Only this pid's windows."},
                    "onScreenOnly": {"type": "boolean", "description": "Drop windows not on-screen. Default false."},
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_launch_app",
            "description": "Launch an application WITHOUT stealing focus (starts unactivated; startMinimized keeps it minimized). Provide name, path, bundleId/aumid, launchPath (exact string from computer_list_apps — most reliable), or urls. Returns the new pid — follow with computer_wait{for:{window:true}} before observing it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "App display name."},
                    "path": {"type": "string", "description": "Full executable path (Windows) or .app path (macOS)."},
                    "bundleId": {"type": "string"},
                    "aumid": {"type": "string", "description": "Windows packaged-app AUMID."},
                    "launchPath": {"type": "string", "description": "Exact launch string from computer_list_apps — the most reliable selector."},
                    "args": {"type": "array", "items": {"type": "string"}},
                    "urls": {"type": "array", "items": {"type": "string"}, "description": "Open URLs in the default browser, not activated."},
                    "startMinimized": {"type": "boolean", "description": "Keep the new window minimized (background automation)."},
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_close_app",
            "description": "Force-terminate a process by pid. Prefer closing the window normally first (a click on the window's Close element, or Alt+F4 via foreground); use this when polite close fails — unsaved state is lost.",
            "inputSchema": {
                "type": "object",
                "properties": {"pid": {"type": "integer"}},
                "required": ["pid"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_focus_app",
            "description": "Raise an app's window to the OS foreground and LEAVE it there — this changes what the user is looking at. Requires Full Control; prefer action-scoped deliveryMode:'foreground' over a persistent focus change when you only need one input to land.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {"type": "integer"},
                    "windowId": {"type": "integer"},
                },
                "required": ["pid"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_click",
            "description": "Click an element or pixel in a window. PREFER element refs: elementIndex+snapshotId+windowId or elementToken — they work on backgrounded/minimized windows and survive DPI/scaling. x/y are window-local pixels from the observe digest/screenshot. 'expect' optionally runs computer_verify after the click. Returns the driver's effect report (confirmed|partial|unverifiable|suspected_noop) — 'unverifiable'/'suspected_noop' means re-observe before assuming success. On stale_element: re-observe and pick a fresh ref. On background_unavailable: retry with deliveryMode:'foreground' only if Full Control is on.",
            "inputSchema": click_schema(),
        }),
        json!({
            "name": "computer_right_click",
            "description": "Right-click an element (context menus). Same targeting, refs, delivery and verification rules as computer_click — follow with computer_observe or computer_wait to see the menu.",
            "inputSchema": click_schema(),
        }),
        json!({
            "name": "computer_double_click",
            "description": "Double-click an element (open files/items, word select). Same targeting, refs, delivery and verification rules as computer_click.",
            "inputSchema": click_schema(),
        }),
        json!({
            "name": "computer_type_text",
            "description": "Type text into an element or at a pixel — character-by-character delivery with no focus steal in background mode. Prefer targeting a focused edit element (elementIndex/elementToken). For Enter/Escape/Tab/arrows use computer_press_key; for whole-field replacement prefer computer_set_value.",
            "inputSchema": {
                "type": "object",
                "properties": target_props(json!({
                    "text": {"type": "string", "description": "Text to insert."},
                    "delayMs": {"type": "integer", "description": "Delay between characters (0-200, default 30)."},
                    "expect": {"type": "array", "description": "Optional computer_verify predicates to run after typing."},
                })),
                "required": ["text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_set_value",
            "description": "Set a value DIRECTLY on an element (text fields, sliders, combo boxes) — the most reliable write path when the element exposes a value. Prefer over keystrokes: no focus, no partial-typing states. Needs elementIndex+snapshotId or elementToken plus pid+windowId.",
            "inputSchema": {
                "type": "object",
                "properties": target_props(json!({
                    "value": {"type": "string"},
                    "expect": {"type": "array", "description": "Optional computer_verify predicates to run after."},
                })),
                "required": ["value", "pid"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_press_key",
            "description": "Send ONE key to a window without focusing it — return|escape|tab|space|delete|backspace|arrows|home|end|pageup|pagedown|f1-f12|letters|digits, with optional 'modifiers' ([ctrl,shift,alt,win/cmd]). For combos like Ctrl+S use computer_hotkey.",
            "inputSchema": {
                "type": "object",
                "properties": target_props(json!({
                    "key": {"type": "string", "description": "Single key name."},
                    "modifiers": {"type": "array", "items": {"type": "string"}},
                    "expect": {"type": "array", "description": "Optional computer_verify predicates to run after."},
                })),
                "required": ["key"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_hotkey",
            "description": "Send a key COMBINATION to a window — keys like [\"ctrl\",\"s\"], [\"ctrl\",\"shift\",\"p\"], [\"alt\",\"f4\"]. Prefer menu equivalents via computer_invoke_menu when the app exposes them (more reliable than chords).",
            "inputSchema": {
                "type": "object",
                "properties": target_props(json!({
                    "keys": {"type": "array", "items": {"type": "string"}, "description": "Key combination, modifiers first, e.g. [\"ctrl\",\"c\"]."},
                    "expect": {"type": "array", "description": "Optional computer_verify predicates to run after."},
                })),
                "required": ["keys"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_scroll",
            "description": "Scroll a window (or element region) in a direction by 'amount' ticks ('by': line|page, default line×3). The scroll goes to the focused/target region — for a specific container, prefer computer_scroll_until which re-observes each step.",
            "inputSchema": {
                "type": "object",
                "properties": target_props(json!({
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                    "amount": {"type": "integer", "description": "Ticks (default 3)."},
                    "by": {"type": "string", "enum": ["line", "page"], "description": "Granularity (default line)."},
                })),
                "required": ["direction"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_scroll_until",
            "description": "Scroll a window until 'query' appears — the mechanical part Cosmos handles for you. Each step scrolls, re-observes and re-searches the FULL element set; stops on a match (returns its E-ref), the scroll edge (content stopped changing), or maxSteps (default 8, cap 20). Use instead of issuing manual scroll+observe loops.",
            "inputSchema": {
                "type": "object",
                "properties": target_props(json!({
                    "query": {"type": "string", "description": "Text to find while scrolling (same matching as computer_find)."},
                    "role": {"type": "string", "description": "Optional role filter for the match."},
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"], "description": "Default 'down'."},
                    "by": {"type": "string", "enum": ["line", "page"], "description": "Granularity per step (default 'page')."},
                    "maxSteps": {"type": "integer", "description": "Scroll steps before giving up (default 8, cap 20)."},
                })),
                "required": ["query"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_drag",
            "description": "Drag from (fromX,fromY) to (toX,toY) in window-local pixels — file drops, reordering, sliders, selections. durationMs/steps shape the gesture (default 500ms/20 steps — slow drags register better). Title-bar/window drags need deliveryMode:'foreground' and Full Control. Verify the result with computer_observe/verify — drops can silently miss.",
            "inputSchema": {
                "type": "object",
                "properties": target_props(json!({
                    "fromX": {"type": "number"}, "fromY": {"type": "number"},
                    "toX": {"type": "number"}, "toY": {"type": "number"},
                    "durationMs": {"type": "integer", "description": "Gesture wall-clock ms (default 500)."},
                    "steps": {"type": "integer", "description": "Intermediate move events (default 20)."},
                })),
                "required": ["fromX", "fromY", "toX", "toY"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_invoke_menu",
            "description": "Invoke a native menu command by path — 'File > Save', 'Edit > Copy', 'View > Zoom In'. More reliable than hotkey chords when the app exposes real menus (menus, menu bars, context menus the driver can resolve). Returns the driver's effect report; verify the outcome with computer_observe/verify.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Target process id."},
                    "windowId": {"type": "integer", "description": "Target window handle (defaults to the pid's frontmost)."},
                    "path": {"type": "string", "description": "Menu path, e.g. 'File > Save As'."},
                    "deliveryMode": {"type": "string", "enum": ["background", "foreground"]},
                    "expect": {"type": "array", "description": "Optional computer_verify predicates to run after."},
                },
                "required": ["pid", "path"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_set_window_frame",
            "description": "Move/resize a window exactly — x,y,width,height in desktop coordinates (same space as computer_list_windows bounds). Deterministic and verified by geometry readback. Use to arrange windows for a task instead of title-bar drags.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "Target process id."},
                    "windowId": {"type": "integer", "description": "Target window handle."},
                    "x": {"type": "number"}, "y": {"type": "number"},
                    "width": {"type": "number"}, "height": {"type": "number"},
                },
                "required": ["pid", "x", "y", "width", "height"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_clipboard_read",
            "description": "Read the system clipboard. Default returns available content TYPES only; includeText also returns plain-text content — privacy-sensitive, ask only when the task needs it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "includeText": {"type": "boolean", "description": "Also return plain-text clipboard content (privacy-sensitive)."},
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_clipboard_write",
            "description": "Replace the system clipboard with ONE value: 'text', 'imagePath' (absolute local image), or 'filePath' (absolute local file as a file URL — the file-drop primitive for attaching files in apps). Follow with a paste action in the target window.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "imagePath": {"type": "string", "description": "Absolute path to a local image."},
                    "filePath": {"type": "string", "description": "Absolute path to a local file (becomes a file URL)."},
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_recording",
            "description": "Control your own session's action recording. action 'start' begins a session-scoped trajectory capture (per-action before/after state + screenshots + a screen video finalized on 'stop') written to outputDir; 'stop' finalizes it; 'status' reports state. Only use this when the user asked to record — it is off by default.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["start", "stop", "status"], "description": "What to do."},
                    "outputDir": {"type": "string", "description": "For 'start': absolute directory for turn folders + recording.mp4 (created if missing)."},
                    "recordVideo": {"type": "boolean", "description": "For 'start': capture the main display to recording.mp4 (default true)."},
                },
                "required": ["action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "computer_move_cursor",
            "description": "Move the AGENT'S OWN overlay cursor to window-local (x,y) — the visible pointer the user sees as 'the agent's hand'. Never the user's physical cursor. Optional; visual feedback only.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": {"type": "number"}, "y": {"type": "number"},
                    "target": {"type": "object", "description": "Optional driver target {kind:'window',pid,window_id}."},
                },
                "required": ["x", "y"],
                "additionalProperties": false,
            },
        }),
    ]
}

/// The shared addressing/delivery properties every pointer/key action takes.
fn target_props(extra: Value) -> serde_json::Map<String, Value> {
    let mut props = serde_json::Map::new();
    props.insert(
        "pid".into(),
        json!({"type": "integer", "description": "Target process id (required unless 'target' carries it)."}),
    );
    props.insert("windowId".into(), json!({"type": "integer", "description": "Target window handle (defaults to the pid's frontmost window)."}));
    props.insert("elementToken".into(), json!({"type": "string", "description": "Snapshot-bound element ref like 's0000002a:14' — the robust way to address a control; survives backgrounding."}));
    props.insert("elementIndex".into(), json!({"type": "integer", "description": "E{n} from the observe/find digest — requires windowId + snapshotId."}));
    props.insert(
        "snapshotId".into(),
        json!({"type": "string", "description": "Snapshot the elementIndex belongs to (from the digest header)."}),
    );
    props.insert(
        "x".into(),
        json!({"type": "number", "description": "Window-local pixel X (from element bounds or the screenshot)."}),
    );
    props.insert(
        "y".into(),
        json!({"type": "number", "description": "Window-local pixel Y."}),
    );
    props.insert("scope".into(), json!({"type": "string", "enum": ["window", "desktop"], "description": "'desktop' = absolute screen pixels targeting the foreground app — last resort."}));
    props.insert(
        "target".into(),
        json!({"type": "object", "description": "Explicit driver target {kind:'window'|'desktop',...} (advanced)."}),
    );
    props.insert("deliveryMode".into(), json!({"type": "string", "enum": ["background", "foreground"], "description": "'background' (default) never steals focus. 'foreground' = brief focus swap, restored after — only after a background_unavailable error AND with Full Control enabled."}));
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            props.insert(key.clone(), value.clone());
        }
    }
    props
}

fn click_schema() -> Value {
    json!({
        "type": "object",
        "properties": target_props(json!({
            "button": {"type": "string", "enum": ["left", "middle"], "description": "Button for computer_click (default left)."},
            "count": {"type": "integer", "description": "Click count 1-3 (default 1)."},
            "modifiers": {"type": "array", "items": {"type": "string"}, "description": "Held modifiers: ctrl, shift, alt, win/cmd."},
            "expect": {"type": "array", "description": "Optional computer_verify predicates to run after the click."},
        })),
        "additionalProperties": false,
    })
}

/// Does `name` act on the desktop (vs inspect)? The service gates these by
/// the control mode.
pub fn is_action_tool(name: &str) -> bool {
    matches!(
        name,
        "computer_launch_app"
            | "computer_close_app"
            | "computer_focus_app"
            | "computer_click"
            | "computer_right_click"
            | "computer_double_click"
            | "computer_type_text"
            | "computer_set_value"
            | "computer_press_key"
            | "computer_hotkey"
            | "computer_scroll"
            | "computer_scroll_until"
            | "computer_drag"
            | "computer_invoke_menu"
            | "computer_set_window_frame"
            | "computer_clipboard_write"
            | "computer_move_cursor"
    )
}

/// Tools that require Full Control regardless of deliveryMode — they change
/// what the user is looking at (persistent foreground) rather than borrowing
/// it briefly for one action.
pub fn requires_full_control(name: &str) -> bool {
    matches!(name, "computer_focus_app")
}
