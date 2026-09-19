//! Linux X11 window inventory that does not walk AT-SPI.
//!
//! `cua-driver mcp` (stdio) can stall for a full call timeout on a GPUI
//! window or a wedged accessibility walk. The X11 client list (`wmctrl`) and
//! a healthy `cua-driver serve` socket both enumerate the **same** window ids
//! Cua uses for observe/act. Cosmos Computer uses this as the fast path for
//! `computer_list_windows` so an OpenCode agent never waits 60s on a stuck
//! stdio child. Agents still only see Cosmos MCP tools — this is not a
//! shell-automation escape hatch.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::process::Command;

use crate::backend::{ActionOutcome, ActionRequest};
use crate::{ComputerError, StagedScreenshot};

const WMCTRL_TIMEOUT: Duration = Duration::from_secs(4);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(8);
const INPUT_TIMEOUT: Duration = Duration::from_secs(5);

/// Parse `wmctrl -lpG` stdout into Cua-shaped window records.
///
/// Columns: `id desktop pid x y w h host title…`
pub fn parse_wmctrl(stdout: &str) -> Vec<Value> {
    let mut windows = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(id_hex) = parts.next() else { continue };
        let Some(desktop) = parts.next() else {
            continue;
        };
        let Some(pid_s) = parts.next() else { continue };
        let Some(x_s) = parts.next() else { continue };
        let Some(y_s) = parts.next() else { continue };
        let Some(w_s) = parts.next() else { continue };
        let Some(h_s) = parts.next() else { continue };
        let Some(_host) = parts.next() else { continue };
        let title = parts.collect::<Vec<_>>().join(" ");
        if title == "N/A" {
            // Cosmos GPUI itself — listing it is fine, but AT-SPI walks hang.
            // Keep it out of the default inventory so agents pin real apps.
            continue;
        }
        let Ok(window_id) = u64::from_str_radix(id_hex.trim_start_matches("0x"), 16) else {
            continue;
        };
        let Ok(pid) = pid_s.parse::<i64>() else {
            continue;
        };
        let x = x_s.parse::<i64>().unwrap_or(0);
        let y = y_s.parse::<i64>().unwrap_or(0);
        let width = w_s.parse::<i64>().unwrap_or(0);
        let height = h_s.parse::<i64>().unwrap_or(0);
        let desktop_n = desktop.parse::<i64>().unwrap_or(0);
        windows.push(json!({
            "app_name": app_name_from_title(&title),
            "title": title,
            "pid": pid,
            "window_id": window_id as i64,
            "windowId": window_id as i64,
            "x": x,
            "y": y,
            "width": width,
            "height": height,
            "bounds": { "x": x, "y": y, "width": width, "height": height },
            "is_on_screen": desktop_n >= 0 && width > 0 && height > 0,
            "minimized": desktop_n < 0 && title != "xfce4-panel" && title != "Desktop" && title != "plank",
            "z_index": windows.len() as i64,
            "source": "x11-wmctrl",
        }));
    }
    windows
}

fn app_name_from_title(title: &str) -> String {
    if let Some((_, rest)) = title.rsplit_once(" - ") {
        return rest.trim().to_string();
    }
    match title {
        "xfce4-panel" => "Xfce4-panel".into(),
        "Desktop" => "Xfdesktop".into(),
        "plank" => "Plank".into(),
        "Color Profiles" => "Xfce4-settings-manager".into(),
        other => other.to_string(),
    }
}

fn filter_windows(mut windows: Vec<Value>, pid: Option<i64>, on_screen_only: bool) -> Vec<Value> {
    if let Some(pid) = pid {
        windows.retain(|w| w.get("pid").and_then(Value::as_i64) == Some(pid));
    }
    if on_screen_only {
        windows.retain(|w| w.get("is_on_screen").and_then(Value::as_bool) == Some(true));
    }
    windows
}

async fn wmctrl_windows() -> Result<Vec<Value>, ComputerError> {
    let child = Command::new("wmctrl")
        .args(["-lpG"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ComputerError::Unavailable(format!("wmctrl: {e}")))?;
    let output = tokio::time::timeout(WMCTRL_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ComputerError::Unavailable("wmctrl timed out".into()))?
        .map_err(|e| ComputerError::Unavailable(format!("wmctrl: {e}")))?;
    if !output.status.success() {
        return Err(ComputerError::Unavailable("wmctrl failed".into()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_wmctrl(&stdout))
}

/// `cua-driver call list_windows --socket` — same ids as observe, no stdio MCP.
async fn socket_windows(socket: &Path, binary: &Path) -> Result<Vec<Value>, ComputerError> {
    let child = Command::new(binary)
        .args([
            "call",
            "list_windows",
            "--socket",
            &socket.display().to_string(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ComputerError::Unavailable(format!("cua-driver call: {e}")))?;
    let output = tokio::time::timeout(SOCKET_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ComputerError::Unavailable("cua-driver socket list_windows timed out".into()))?
        .map_err(|e| ComputerError::Unavailable(format!("cua-driver call: {e}")))?;
    if !output.status.success() {
        return Err(ComputerError::Unavailable(
            "cua-driver socket list_windows failed".into(),
        ));
    }
    let parsed: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| ComputerError::Unavailable(format!("socket json: {e}")))?;
    let windows = parsed
        .get("windows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(windows)
}

fn default_socket() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = Path::new(&home)
        .join(".cache")
        .join("cua-driver")
        .join("cua-driver.sock");
    path.exists().then_some(path)
}

fn default_binary() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = Path::new(&home)
        .join(".zeron")
        .join("computer")
        .join("cua-driver")
        .join("versions")
        .join("0.28.2-x86_64-unknown-linux-gnu")
        .join("cua-driver");
    path.exists().then_some(path)
}

/// Best-effort Linux window list. Prefer `wmctrl` (X11 client list, ~ms) so a
/// wedged `cua-driver mcp` stdio child cannot stall inventory — including
/// per-pid lookups. GTK file choosers that have their own X11 window (e.g.
/// "Open File") appear here; a11y-only sheets still require the driver.
pub async fn list_windows(pid: Option<i64>, on_screen_only: bool) -> Result<Value, ComputerError> {
    if let Ok(windows) = wmctrl_windows().await {
        let windows = filter_windows(windows, pid, on_screen_only);
        return Ok(json!({ "windows": windows, "source": "x11-wmctrl" }));
    }
    if let (Some(socket), Some(binary)) = (default_socket(), default_binary())
        && let Ok(windows) = socket_windows(&socket, &binary).await
    {
        let windows = filter_windows(windows, pid, on_screen_only);
        return Ok(json!({ "windows": windows, "source": "cua-serve-socket" }));
    }
    Err(ComputerError::Unavailable(
        "no X11 window inventory (wmctrl and cua-driver socket both failed)".into(),
    ))
}

/// Fast observe fallback when Cua `get_window_state` times out. Titles and
/// bounds only — no AT-SPI tree. Honest about the missing semantic walk.
pub async fn window_record(pid: i64, window_id: Option<i64>) -> Result<Value, ComputerError> {
    let windows = wmctrl_windows().await?;
    let found = windows.into_iter().find(|w| {
        w.get("pid").and_then(Value::as_i64) == Some(pid)
            && window_id
                .map(|id| w.get("window_id").and_then(Value::as_i64) == Some(id))
                .unwrap_or(true)
    });
    found.ok_or_else(|| ComputerError::Unavailable(format!("no X11 window for pid {pid}")))
}

/// Compact observation from a wmctrl window record — no AT-SPI wait.
pub fn observation_from_record(record: &Value) -> crate::Observation {
    let title = record
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("window");
    let app = record
        .get("app_name")
        .and_then(Value::as_str)
        .unwrap_or("app");
    let pid = record.get("pid").and_then(Value::as_i64);
    let window_id = record
        .get("windowId")
        .or_else(|| record.get("window_id"))
        .and_then(Value::as_i64);
    let text = format!(
        "X11 window app={app} title={title} pid={pid:?} windowId={window_id:?}\n\
         Semantic AT-SPI tree skipped (GTK/Qt inspect hangs this session). Keyboard/click tools use the X11 input path when Cua is wedged."
    );
    crate::Observation {
        scope: "window".into(),
        window: Some(crate::ObservedWindow {
            app_name: Some(app.into()),
            window_title: Some(title.into()),
            bundle_id: None,
            pid,
            window_id,
        }),
        text,
        truncated: false,
        screenshot: None,
        modality: "semantic".into(),
    }
}

async fn run_cmd(program: &str, args: &[&str]) -> Result<String, ComputerError> {
    let child = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ComputerError::Unavailable(format!("{program}: {e}")))?;
    let output = tokio::time::timeout(INPUT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ComputerError::Unavailable(format!("{program} timed out")))?
        .map_err(|e| ComputerError::Unavailable(format!("{program}: {e}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(ComputerError::Unavailable(format!(
            "{program} failed: {err}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn hex_window(id: i64) -> String {
    format!("0x{id:x}")
}

/// Map Cosmos `computer_hotkey` tokens to xdotool key names.
pub fn xdotool_key(token: &str) -> String {
    match token.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "ctl" => "ctrl".into(),
        "alt" | "option" => "alt".into(),
        "shift" => "shift".into(),
        "super" | "meta" | "win" | "cmd" => "super".into(),
        "escape" | "esc" => "Escape".into(),
        "enter" | "return" => "Return".into(),
        "tab" => "Tab".into(),
        "space" => "space".into(),
        "backspace" => "BackSpace".into(),
        "delete" | "del" => "Delete".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" | "page_up" => "Page_Up".into(),
        "pagedown" | "page_down" => "Page_Down".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "f1" => "F1".into(),
        "f2" => "F2".into(),
        "f3" => "F3".into(),
        "f4" => "F4".into(),
        "f5" => "F5".into(),
        "f6" => "F6".into(),
        "f7" => "F7".into(),
        "f8" => "F8".into(),
        "f9" => "F9".into(),
        "f10" => "F10".into(),
        "f11" => "F11".into(),
        "f12" => "F12".into(),
        other if other.len() == 1 => other.to_string(),
        other => other.to_string(),
    }
}

pub fn hotkey_chord(keys: &[String]) -> String {
    keys.iter().map(|k| xdotool_key(k)).collect::<Vec<_>>().join("+")
}

async fn activate(window_id: i64) -> Result<(), ComputerError> {
    let hex = hex_window(window_id);
    let _ = run_cmd("wmctrl", &["-i", "-a", &hex]).await;
    Ok(())
}

async fn window_id_from_args(args: &Value) -> Option<i64> {
    if let Some(id) = args.get("windowId").and_then(Value::as_i64) {
        return Some(id);
    }
    let pid = args.get("pid").and_then(Value::as_i64)?;
    window_record(pid, None)
        .await
        .ok()?
        .get("windowId")
        .and_then(Value::as_i64)
}

/// Capture one X11 window (or the root) to `dest` with ImageMagick `import`.
pub async fn capture_window(
    window_id: Option<i64>,
    dest: &Path,
) -> Result<StagedScreenshot, ComputerError> {
    let dest_s = dest.to_string_lossy().into_owned();
    if let Some(id) = window_id {
        let hex = hex_window(id);
        run_cmd("import", &["-window", &hex, &dest_s]).await?;
    } else {
        run_cmd("import", &["-window", "root", &dest_s]).await?;
    }
    let meta = tokio::fs::metadata(dest)
        .await
        .map_err(|e| ComputerError::Unavailable(format!("screenshot: {e}")))?;
    Ok(StagedScreenshot {
        path: dest.to_path_buf(),
        width: None,
        height: None,
        bytes: meta.len(),
    })
}

/// True when this tool can run on X11 without an accessibility token.
pub fn can_handle(tool: &str, args: &Value) -> bool {
    match tool {
        "computer_type_text" => args.get("text").and_then(Value::as_str).is_some(),
        "computer_hotkey" => args.get("keys").and_then(Value::as_array).is_some(),
        "computer_press_key" => args.get("key").and_then(Value::as_str).is_some(),
        "computer_set_window_frame"
        | "computer_focus_app"
        | "computer_clipboard_read"
        | "computer_clipboard_write"
        | "computer_scroll" => true,
        "computer_click" | "computer_double_click" | "computer_right_click" => {
            args.get("elementToken").is_none() && args.get("elementIndex").is_none()
        }
        _ => false,
    }
}

/// Fast Linux input path. Agents still only call Cosmos `computer_*` tools.
pub async fn try_act(request: &ActionRequest) -> Option<ActionOutcome> {
    if !can_handle(&request.tool, &request.args) {
        return None;
    }
    match act(request).await {
        Ok(outcome) => Some(outcome),
        Err(_) => None,
    }
}

async fn act(request: &ActionRequest) -> Result<ActionOutcome, ComputerError> {
    let args = &request.args;
    let window_id = window_id_from_args(args).await;
    if let Some(id) = window_id {
        activate(id).await?;
    }
    let summary = match request.tool.as_str() {
        "computer_type_text" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| ComputerError::InvalidArgs("text required".into()))?;
            run_cmd(
                "xdotool",
                &["type", "--clearmodifiers", "--delay", "8", "--", text],
            )
            .await?;
            format!("typed {} chars via X11", text.chars().count())
        }
        "computer_hotkey" => {
            let keys = args
                .get("keys")
                .and_then(Value::as_array)
                .ok_or_else(|| ComputerError::InvalidArgs("keys array required".into()))?;
            let tokens: Vec<String> = keys
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let chord = hotkey_chord(&tokens);
            run_cmd("xdotool", &["key", "--clearmodifiers", &chord]).await?;
            format!("hotkey {chord} via X11")
        }
        "computer_press_key" => {
            let key = args
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| ComputerError::InvalidArgs("key required".into()))?;
            let mapped = xdotool_key(key);
            run_cmd("xdotool", &["key", "--clearmodifiers", &mapped]).await?;
            format!("press {mapped} via X11")
        }
        "computer_focus_app" => {
            let id = window_id.ok_or_else(|| {
                ComputerError::InvalidArgs("focus needs pid or windowId".into())
            })?;
            format!("focused window {} via wmctrl", hex_window(id))
        }
        "computer_set_window_frame" => {
            let id = window_id.ok_or_else(|| {
                ComputerError::InvalidArgs("set_window_frame needs pid or windowId".into())
            })?;
            let x = args.get("x").and_then(Value::as_i64).unwrap_or(0);
            let y = args.get("y").and_then(Value::as_i64).unwrap_or(0);
            let w = args.get("width").and_then(Value::as_i64).unwrap_or(800);
            let h = args.get("height").and_then(Value::as_i64).unwrap_or(600);
            let spec = format!("0,{x},{y},{w},{h}");
            let hex = hex_window(id);
            run_cmd("wmctrl", &["-i", "-r", &hex, "-e", &spec]).await?;
            format!("moved window {hex} to {spec}")
        }
        "computer_click" | "computer_double_click" | "computer_right_click" => {
            let button = match request.tool.as_str() {
                "computer_right_click" => "3",
                _ => "1",
            };
            let count = if request.tool == "computer_double_click" {
                2
            } else {
                1
            };
            let mut argv: Vec<String> = Vec::new();
            if let (Some(x), Some(y)) = (
                args.get("x").and_then(Value::as_f64),
                args.get("y").and_then(Value::as_f64),
            ) {
                argv.extend([
                    "mousemove".into(),
                    "--".into(),
                    format!("{}", x as i64),
                    format!("{}", y as i64),
                ]);
            }
            argv.extend([
                "click".into(),
                "--repeat".into(),
                count.to_string(),
                button.into(),
            ]);
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            run_cmd("xdotool", &refs).await?;
            format!("click button={button} count={count} via X11")
        }
        "computer_scroll" => {
            let dir = args
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("down");
            let button = match dir {
                "up" => "4",
                "left" => "6",
                "right" => "7",
                _ => "5",
            };
            let amount = args.get("amount").and_then(Value::as_i64).unwrap_or(3).max(1);
            for _ in 0..amount.min(20) {
                run_cmd("xdotool", &["click", button]).await?;
            }
            format!("scroll {dir} x{amount} via X11")
        }
        "computer_clipboard_write" => {
            let text = args.get("text").and_then(Value::as_str).unwrap_or("");
            let mut child = Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| ComputerError::Unavailable(format!("xclip: {e}")))?;
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(text.as_bytes()).await;
            }
            let _ = tokio::time::timeout(INPUT_TIMEOUT, child.wait()).await;
            format!("clipboard wrote {} bytes", text.len())
        }
        "computer_clipboard_read" => {
            let text = run_cmd("xclip", &["-selection", "clipboard", "-o"]).await?;
            return Ok(ActionOutcome {
                summary: format!("clipboard {} chars", text.chars().count()),
                details: Some(json!({ "text": text, "source": "x11-xclip" })),
                observe: None,
            });
        }
        other => {
            return Err(ComputerError::Unavailable(format!(
                "x11 cannot handle {other}"
            )));
        }
    };
    Ok(ActionOutcome {
        summary,
        details: Some(json!({ "source": "x11-input", "windowId": window_id })),
        observe: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
0x01000003 -1 2272   0    0    1920 29   cursor xfce4-panel
0x0140070c  0 2278   641  416  640  480  cursor task1 - Thunar
0x02400003  0 30271  641  416  640  480  cursor /tmp/a.txt - Mousepad
0x02600007  0 30462  641  378  640  500  cursor Color Profiles
0x01a00001  0 47105  218  101  1320 880     N/A N/A
";

    #[test]
    fn wmctrl_parse_skips_gpui_and_keeps_ids() {
        let windows = parse_wmctrl(SAMPLE);
        assert_eq!(windows.len(), 4);
        let thunar = windows
            .iter()
            .find(|w| w["title"].as_str() == Some("task1 - Thunar"))
            .unwrap();
        assert_eq!(thunar["pid"], 2278);
        assert_eq!(thunar["window_id"], 0x0140_070c_i64);
        assert_eq!(thunar["windowId"], 0x0140_070c_i64);
        assert_eq!(thunar["app_name"], "Thunar");
        assert!(windows.iter().all(|w| w["title"].as_str() != Some("N/A")));
    }

    #[test]
    fn pid_filter() {
        let filtered = filter_windows(parse_wmctrl(SAMPLE), Some(30271), false);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["app_name"], "Mousepad");
    }

    #[test]
    fn window_record_matches_pid() {
        let windows = parse_wmctrl(SAMPLE);
        let mousepad = windows
            .iter()
            .find(|w| w.get("pid").and_then(Value::as_i64) == Some(30271))
            .unwrap();
        assert_eq!(mousepad["title"].as_str(), Some("/tmp/a.txt - Mousepad"));
        assert_eq!(mousepad["windowId"], 0x0240_0003_i64);
    }

    #[test]
    fn hotkey_chord_is_an_array_not_a_plus_string() {
        assert_eq!(xdotool_key("ctrl"), "ctrl");
        assert_eq!(xdotool_key("s"), "s");
        assert_eq!(xdotool_key("Escape"), "Escape");
        assert_eq!(xdotool_key("f4"), "F4");
        assert_eq!(hotkey_chord(&["ctrl".into(), "s".into()]), "ctrl+s");
        assert_eq!(hotkey_chord(&["escape".into()]), "Escape");
        assert_eq!(hotkey_chord(&["alt".into(), "F4".into()]), "alt+F4");
        assert!(can_handle(
            "computer_hotkey",
            &json!({"keys": ["ctrl", "s"]})
        ));
        assert!(!can_handle(
            "computer_click",
            &json!({"elementToken": "s1:2"})
        ));
        assert!(can_handle("computer_click", &json!({"x": 10, "y": 10})));
    }
}
