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

use crate::ComputerError;

const WMCTRL_TIMEOUT: Duration = Duration::from_secs(4);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(8);

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
/// wedged `cua-driver mcp` stdio child cannot stall **global** inventory.
/// Per-pid lookups stay on the Cua driver so transient GTK dialogs that share
/// an X11 window still appear as distinct accessibility windows.
pub async fn list_windows(pid: Option<i64>, on_screen_only: bool) -> Result<Value, ComputerError> {
    if pid.is_some() {
        return Err(ComputerError::Unavailable(
            "per-pid window list uses the driver".into(),
        ));
    }
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
}
