//! Seat desktop — computer use for cloud agents.
//!
//! Railway, Boat and BYO-VPS seats are headless Linux boxes. To let the agent
//! running ON the seat see and drive a screen (and let the desktop's Computer
//! pane watch and take control), each seat gets:
//!
//! - a virtual display (`Xvfb :99`, 1280×800) plus a light window manager
//!   when one is installable — started by [`SETUP_SCRIPT`], detached, so a
//!   run never waits on package installs;
//! - `~/.cosmos/bin/cosmos-computer` ([`CLI_SCRIPT`]) — one small POSIX
//!   script the agent calls from its shell tool: `screenshot`, `click`,
//!   `type`, `key`, `scroll`, `drag`, `open <url>`, `record start|stop`, …
//!   backed by `xdotool` + ImageMagick/scrot/ffmpeg;
//! - a short brief prepended to the prompt ([`agent_brief`]) naming the tool,
//!   so any harness (Claude Code, OpenCode, Codex, …) can use it without an
//!   MCP server on the seat.
//!
//! The desktop app drives the same display through [`SeatAction`] →
//! `cosmos-computer` over the seat's exec surface: no second computer-use
//! stack, no embedded VNC, nothing new in the binary.

use serde::{Deserialize, Serialize};

use crate::cloud::shell_word;

/// The seat's virtual display.
pub const DISPLAY: &str = ":99";
/// Where screenshots and recordings land on the seat.
pub const SHOTS_DIR: &str = "$HOME/.cosmos/shots";

/// `cosmos-computer` — the seat-side computer-use CLI. POSIX sh so it runs on
/// Debian/Ubuntu/Alpine images alike.
pub const CLI_SCRIPT: &str = r#"#!/bin/sh
# cosmos-computer — drive this cloud seat's virtual desktop (Cosmos).
export DISPLAY="${DISPLAY:-:99}"
C="$HOME/.cosmos"
SHOTS="${COSMOS_SHOTS:-$C/shots}"
mkdir -p "$SHOTS"

die() { echo "cosmos-computer: $*" >&2; exit 1; }
# Background work leaves the caller's session entirely: a harness run rides
# `ssh -tt`, and the pty hangup at its end would otherwise take the
# browser's helper processes (and a recording) down with it.
detach() { if command -v setsid >/dev/null 2>&1; then setsid "$@"; else "$@"; fi; }

# The display is live only while its X server runs: a socket left behind by
# a reboot (or a killed Xvfb) must not read as "ready".
x_up() {
  n="${DISPLAY#:}"; n="${n%%.*}"
  p=$(tr -d ' ' <"/tmp/.X$n-lock" 2>/dev/null)
  [ -n "$p" ] && [ -d "/proc/$p" ] && [ -e "/tmp/.X11-unix/X$n" ]
}

wait_ready() {
  i=0
  while ! x_up; do
    st=$(cat "$C/desktop.status" 2>/dev/null)
    case "$st" in failed*) die "desktop unavailable — $st (see $C/desktop.log)";; esac
    if [ "$i" -eq 0 ] && { [ -z "$st" ] || [ "$st" = ready ]; } && [ -x "$C/desktop-setup.sh" ]; then
      detach nohup sh "$C/desktop-setup.sh" </dev/null >"$C/desktop.log" 2>&1 &
    fi
    i=$((i+1)); [ "$i" -gt 180 ] && die "desktop did not come up in 3 minutes (see $C/desktop.log)"
    sleep 1
  done
}

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is not installed on this seat (desktop setup: $(cat "$C/desktop.status" 2>/dev/null))"; }

capture() {
  out="$1"
  if command -v import >/dev/null 2>&1; then import -window root "$out" >/dev/null 2>&1
  elif command -v scrot >/dev/null 2>&1; then scrot -o "$out" >/dev/null 2>&1
  elif command -v xwd >/dev/null 2>&1 && command -v convert >/dev/null 2>&1; then xwd -root -silent | convert xwd:- "$out"
  elif command -v ffmpeg >/dev/null 2>&1; then ffmpeg -loglevel error -y -f x11grab -i "$DISPLAY" -frames:v 1 "$out"
  else die "no screenshot tool (install imagemagick or scrot)"; fi
  [ -s "$out" ] || die "screenshot failed"
}

geometry() {
  if command -v xdotool >/dev/null 2>&1; then xdotool getdisplaygeometry | tr ' ' 'x'
  elif command -v xdpyinfo >/dev/null 2>&1; then xdpyinfo | awk '/dimensions:/{print $2}'
  else echo 1280x800; fi
}

# Ubuntu ships `chromium-browser`/`firefox` as stubs that only install the
# snap (no snapd in a container) — never pick one of those.
snap_stub() { p=$(command -v "$1") || return 1; head -c 2 "$p" 2>/dev/null | grep -q '#!' && grep -qi snap "$p" 2>/dev/null; }

browser() {
  for b in chromium chromium-browser google-chrome google-chrome-stable firefox firefox-esr; do
    command -v "$b" >/dev/null 2>&1 && ! snap_stub "$b" && { echo "$b"; return 0; }
  done
  for b in "${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}"/chromium-*/chrome-linux*/chrome \
           "$HOME"/.cache/ms-playwright/chromium-*/chrome-linux*/chrome; do
    [ -x "$b" ] && { echo "$b"; return 0; }
  done
  return 1
}

sudo_cmd() { if [ "$(id -u)" = 0 ]; then echo ""; elif command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then echo "sudo -n"; else return 1; fi; }

# First `open` on a bare seat: a distro browser when the package manager has
# a real one, else Playwright's own Chromium (no snap, no root for the
# browser itself). Progress goes to stderr; the open waits for it.
install_browser() {
  echo "cosmos-computer: no browser yet — installing one (first time only, about a minute)…" >&2
  if s=$(sudo_cmd); then
    apt_try() { DEBIAN_FRONTEND=noninteractive $s apt-get install -y -qq --no-install-recommends "$1" >/dev/null 2>&1; }
    if command -v apt-get >/dev/null 2>&1; then
      apt_try chromium || apt_try firefox-esr || {
        $s apt-get update -qq >/dev/null 2>&1
        apt_try chromium || apt_try firefox-esr
      }
    elif command -v apk >/dev/null 2>&1; then $s apk add -q chromium >/dev/null 2>&1
    elif command -v dnf >/dev/null 2>&1; then $s dnf install -y -q chromium >/dev/null 2>&1
    fi
    browser >/dev/null && return 0
  fi
  deps=""; sudo_cmd >/dev/null && deps="--with-deps"
  if command -v npx >/dev/null 2>&1; then
    npx -y playwright install $deps chromium >/dev/null 2>&1
  elif command -v python3 >/dev/null 2>&1; then
    python3 -m pip install -q --user playwright >/dev/null 2>&1 \
      || python3 -m pip install -q --user --break-system-packages playwright >/dev/null 2>&1
    python3 -m playwright install $deps chromium >/dev/null 2>&1
  fi
  browser >/dev/null
}

cmd="$1"; [ $# -gt 0 ] && shift
case "$cmd" in
  status)
    echo "display $DISPLAY · $(cat "$C/desktop.status" 2>/dev/null || echo 'not started')"
    x_up && echo "screen $(geometry)";;
  start) wait_ready; echo "ready · screen $(geometry)";;
  screenshot|shot)
    wait_ready
    out="${1:-$SHOTS/shot-$(date +%Y%m%d-%H%M%S).png}"
    capture "$out"; echo "$out";;
  size) wait_ready; geometry;;
  cursor) wait_ready; need xdotool; xdotool getmouselocation;;
  move) wait_ready; need xdotool; xdotool mousemove --sync "$1" "$2";;
  click)
    wait_ready; need xdotool
    case "${3:-left}" in left|1) b=1;; middle|2) b=2;; right|3) b=3;; *) b=1;; esac
    xdotool mousemove --sync "$1" "$2" click "$b";;
  right_click) wait_ready; need xdotool; xdotool mousemove --sync "$1" "$2" click 3;;
  double_click) wait_ready; need xdotool; xdotool mousemove --sync "$1" "$2" click --repeat 2 --delay 90 1;;
  drag) wait_ready; need xdotool; xdotool mousemove --sync "$1" "$2" mousedown 1 mousemove --sync "$3" "$4" mouseup 1;;
  type) wait_ready; need xdotool; xdotool type --delay 12 -- "$*";;
  key) wait_ready; need xdotool; xdotool key --delay 40 -- "$@";;
  scroll)
    wait_ready; need xdotool
    case "$1" in up) b=4;; down) b=5;; left) b=6;; right) b=7;; *) die "scroll up|down|left|right [n] [x y]";; esac
    [ -n "$3" ] && xdotool mousemove --sync "$3" "$4"
    xdotool click --repeat "${2:-3}" --delay 30 "$b";;
  windows)
    wait_ready
    if command -v wmctrl >/dev/null 2>&1; then wmctrl -l
    else need xdotool; for w in $(xdotool search --onlyvisible --name . 2>/dev/null); do echo "$w $(xdotool getwindowname "$w")"; done; fi;;
  open)
    wait_ready
    b=$(browser) || { install_browser; b=$(browser); } \
      || die "no browser on this seat and the automatic install failed — install chromium or firefox (or: npx playwright install --with-deps chromium)"
    # Container-safe Chromium: no sandbox/GPU (root + no /dev/dri), its own
    # profile, and --test-type to drop the --no-sandbox warning bar.
    case "$b" in *chrom*|*chrome*) set -- --no-sandbox --disable-gpu --disable-dev-shm-usage --test-type \
      --no-first-run --no-default-browser-check --user-data-dir="$C/browser-profile" \
      --disable-session-crashed-bubble --hide-crash-restore-bubble \
      --window-position=0,0 --window-size=1280,800 "$@";; esac
    detach nohup "$b" "$@" </dev/null >"$C/browser.log" 2>&1 &
    echo "opened with $b";;
  record)
    wait_ready
    case "$1" in
      start)
        if ! command -v ffmpeg >/dev/null 2>&1; then
          s=$(sudo_cmd) || die "recording needs ffmpeg (no root to install it)"
          if command -v apt-get >/dev/null 2>&1; then DEBIAN_FRONTEND=noninteractive $s apt-get install -y -qq ffmpeg >/dev/null 2>&1 || { $s apt-get update -qq >/dev/null 2>&1; DEBIAN_FRONTEND=noninteractive $s apt-get install -y -qq ffmpeg >/dev/null 2>&1; }
          elif command -v apk >/dev/null 2>&1; then $s apk add -q ffmpeg >/dev/null 2>&1
          elif command -v dnf >/dev/null 2>&1; then $s dnf install -y -q ffmpeg >/dev/null 2>&1; fi
          need ffmpeg
        fi
        [ -f "$C/record.pid" ] && kill -0 "$(cat "$C/record.pid")" 2>/dev/null && die "already recording ($(cat "$C/record.path"))"
        out="${2:-$SHOTS/recording-$(date +%Y%m%d-%H%M%S).mp4}"
        # ffmpeg records its own pid (exec keeps it) — `$!` of a detached
        # job can be a wrapper's.
        rm -f "$C/record.pid"
        detach nohup sh -c 'echo $$ >"$0"; exec "$@"' "$C/record.pid" \
          ffmpeg -nostdin -loglevel error -y -f x11grab -framerate 12 -video_size "$(geometry)" -i "$DISPLAY" \
          -pix_fmt yuv420p -vcodec libx264 -preset veryfast "$out" </dev/null >"$C/record.log" 2>&1 &
        i=0; while [ ! -s "$C/record.pid" ] && [ "$i" -lt 20 ]; do sleep 0.1; i=$((i+1)); done
        echo "$out" >"$C/record.path"; echo "recording → $out";;
      stop)
        [ -f "$C/record.pid" ] || die "not recording"
        kill -INT "$(cat "$C/record.pid")" 2>/dev/null
        i=0; while kill -0 "$(cat "$C/record.pid")" 2>/dev/null && [ "$i" -lt 20 ]; do sleep 0.5; i=$((i+1)); done
        rm -f "$C/record.pid"; cat "$C/record.path";;
      *) die "record start [file.mp4] | record stop";;
    esac;;
  help|--help|-h|"")
    cat <<'EOF'
cosmos-computer — drive this seat's virtual desktop (DISPLAY :99, 1280x800)
  screenshot [file.png]          save a PNG (default ~/.cosmos/shots/…) and print its path
  click X Y [left|right|middle]  right_click X Y · double_click X Y · move X Y
  drag X1 Y1 X2 Y2               press at X1,Y1, release at X2,Y2
  type TEXT                      type text into the focused window
  key COMBO...                   xdotool key names: Return ctrl+l ctrl+shift+t Escape
  scroll up|down|left|right [N] [X Y]
  open URL                       launch a browser on the display
  windows · cursor · size · status · start
  record start [file.mp4] · record stop   screen recording (ffmpeg)
EOF
    ;;
  *) die "unknown command '$cmd' — try: cosmos-computer help";;
esac
"#;

/// Idempotent desktop bring-up: install the display tooling when missing
/// (root or passwordless sudo only), start `Xvfb :99` and a window manager.
/// Runs detached; progress lands in `~/.cosmos/desktop.status`
/// (`installing` → `ready` | `failed: <why>`).
pub const SETUP_SCRIPT: &str = r#"#!/bin/sh
C="$HOME/.cosmos"; mkdir -p "$C"
lock="$C/desktop.lock"
if ! mkdir "$lock" 2>/dev/null; then
  # Another bring-up is in flight — a stale lock (>10 min) is reclaimed.
  if [ -n "$(find "$lock" -maxdepth 0 -mmin +10 2>/dev/null)" ]; then rmdir "$lock" 2>/dev/null; mkdir "$lock" 2>/dev/null || exit 0; else exit 0; fi
fi
trap 'rmdir "$lock" 2>/dev/null' EXIT
st() { echo "$*" >"$C/desktop.status"; }
D=""; command -v setsid >/dev/null 2>&1 && D="setsid"
have() { command -v "$1" >/dev/null 2>&1; }
if ! have Xvfb || ! have xdotool || { ! have import && ! have scrot; }; then
  st installing
  if [ "$(id -u)" = 0 ]; then s=""; elif have sudo && sudo -n true 2>/dev/null; then s="sudo -n"; else st "failed: needs root to install Xvfb + xdotool"; exit 1; fi
  if have apt-get; then
    pk="xvfb xdotool imagemagick x11-utils x11-xserver-utils wmctrl openbox"
    DEBIAN_FRONTEND=noninteractive $s apt-get install -y -qq --no-install-recommends $pk >/dev/null 2>&1 || {
      $s apt-get update -qq >/dev/null 2>&1
      DEBIAN_FRONTEND=noninteractive $s apt-get install -y -qq --no-install-recommends $pk >/dev/null 2>&1
    }
  elif have apk; then $s apk add -q xvfb xdotool imagemagick xdpyinfo xsetroot wmctrl openbox font-dejavu >/dev/null 2>&1
  elif have dnf; then $s dnf install -y -q xorg-x11-server-Xvfb xdotool ImageMagick xdpyinfo wmctrl openbox >/dev/null 2>&1
  elif have yum; then $s yum install -y -q xorg-x11-server-Xvfb xdotool ImageMagick >/dev/null 2>&1
  else st "failed: no supported package manager (apt/apk/dnf)"; exit 1; fi
  have Xvfb || { st "failed: couldn't install Xvfb"; exit 1; }
fi
# Live = the lock's pid runs (a reboot leaves the socket and lock behind).
x_up() { p=$(tr -d ' ' </tmp/.X99-lock 2>/dev/null); [ -n "$p" ] && [ -d "/proc/$p" ] && [ -e /tmp/.X11-unix/X99 ]; }
if ! x_up; then
  st starting
  rm -f /tmp/.X99-lock /tmp/.X11-unix/X99 2>/dev/null
  rm -f "$C/wm.pid"
  $D nohup Xvfb :99 -screen 0 1280x800x24 -nolisten tcp -ac </dev/null >"$C/xvfb.log" 2>&1 &
  i=0; while ! x_up && [ "$i" -lt 40 ]; do sleep 0.25; i=$((i+1)); done
  x_up || { st "failed: Xvfb did not start ($(tail -n1 "$C/xvfb.log" 2>/dev/null))"; exit 1; }
fi
export DISPLAY=:99
# Our own WM on :99 (tracked by pid — another display's WM doesn't count).
if ! { [ -f "$C/wm.pid" ] && kill -0 "$(cat "$C/wm.pid")" 2>/dev/null; }; then
  for wm in openbox fluxbox xfwm4 matchbox-window-manager; do
    if have "$wm"; then $D nohup "$wm" </dev/null >"$C/wm.log" 2>&1 & echo $! >"$C/wm.pid"; break; fi
  done
fi
have xsetroot && xsetroot -solid '#1b1d22' 2>/dev/null
st ready
"#;

/// Writes the CLI + setup script to `~/.cosmos` and starts the desktop
/// bring-up detached. Fast (two small writes and a fork) — a run never waits
/// on package installs; `cosmos-computer` itself waits for readiness.
pub fn bootstrap_command() -> String {
    use base64::Engine as _;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    format!(
        "C=\"$HOME/.cosmos\"; mkdir -p \"$C/bin\" \"$C/shots\" && \
printf %s {cli} | base64 -d > \"$C/bin/cosmos-computer\" && chmod +x \"$C/bin/cosmos-computer\" && \
printf %s {setup} | base64 -d > \"$C/desktop-setup.sh\" && chmod +x \"$C/desktop-setup.sh\" && \
x_pid=$(tr -d ' ' </tmp/.X99-lock 2>/dev/null); \
case \"$(cat \"$C/desktop.status\" 2>/dev/null)\" in ready) {{ [ -n \"$x_pid\" ] && [ -d \"/proc/$x_pid\" ] && [ -e /tmp/.X11-unix/X99 ]; }} || $(command -v setsid) nohup sh \"$C/desktop-setup.sh\" </dev/null >\"$C/desktop.log\" 2>&1 & ;; \
installing|starting) ;; *) $(command -v setsid) nohup sh \"$C/desktop-setup.sh\" </dev/null >\"$C/desktop.log\" 2>&1 & ;; esac; echo __cosmos_desktop_ok",
        cli = shell_word(&b64(CLI_SCRIPT)),
        setup = shell_word(&b64(SETUP_SCRIPT)),
    )
}

/// Run the setup synchronously (the Computer pane's "Start desktop"): the
/// bootstrap, then wait for the status file to settle.
pub fn start_command() -> String {
    format!(
        "{} >/dev/null; i=0; while [ \"$i\" -lt 240 ]; do s=$(cat \"$HOME/.cosmos/desktop.status\" 2>/dev/null); \
case \"$s\" in ready|failed*) break;; esac; sleep 1; i=$((i+1)); done; \
echo \"__status=$(cat \"$HOME/.cosmos/desktop.status\" 2>/dev/null)\"",
        bootstrap_command()
    )
}

/// Shell prefix that puts the display and the CLI on the harness's
/// environment.
pub fn env_prefix() -> String {
    format!("export DISPLAY={DISPLAY} PATH=\"$HOME/.cosmos/bin:$PATH\" &&")
}

/// The brief prepended to every seat prompt — what the agent can do on this
/// machine beyond its own tools. Kept short; harnesses see it as the head of
/// the user turn.
pub fn agent_brief(workspace: &str, repo: Option<&str>) -> String {
    let repo_line = match repo {
        Some(repo) => format!(
            "The workspace is a checkout of {repo}. Commit on a branch and open pull requests with `gh` or the GitHub API ($GITHUB_TOKEN is set) when the task calls for it."
        ),
        None => "There is no git remote: the workspace is local to this seat. Cosmos checkpoints it with local git commits and backs it up to the user's computer after every turn, so keep all work inside it.".to_string(),
    };
    format!(
        "<cosmos-seat>\n\
You are running on a Cosmos cloud seat (a Linux machine). Your workspace is {workspace} — work inside it. {repo_line}\n\
This seat has a virtual desktop (DISPLAY=:99, 1280x800) you can see and control from the shell:\n\
  cosmos-computer screenshot            → saves a PNG and prints its path (open it to look)\n\
  cosmos-computer click X Y | double_click X Y | right_click X Y | drag X1 Y1 X2 Y2\n\
  cosmos-computer type \"text\" | key ctrl+l Return | scroll down 5\n\
  cosmos-computer open https://…        → launches a browser on the desktop\n\
  cosmos-computer record start | record stop   → screen recording (mp4)\n\
Use it to verify UI work: run the app, open it, take screenshots, click through it, record a short video. \
Save evidence (screenshots, recordings, test output) under ~/.cosmos/shots — the user sees that folder live.\n\
</cosmos-seat>\n\n"
    )
}

/// One desktop action from the Computer pane's "Take control".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SeatAction {
    Click {
        x: i32,
        y: i32,
        #[serde(default)]
        button: Option<String>,
    },
    DoubleClick {
        x: i32,
        y: i32,
    },
    Move {
        x: i32,
        y: i32,
    },
    Drag {
        x: i32,
        y: i32,
        to_x: i32,
        to_y: i32,
    },
    Type {
        text: String,
    },
    Key {
        combo: String,
    },
    Scroll {
        direction: String,
        #[serde(default)]
        amount: Option<u32>,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
    },
    OpenUrl {
        url: String,
    },
}

fn coord(v: i32) -> Result<String, String> {
    if (0..=10_000).contains(&v) {
        Ok(v.to_string())
    } else {
        Err(format!("coordinate {v} is off the screen"))
    }
}

impl SeatAction {
    /// The `cosmos-computer …` invocation for this action — every argument
    /// validated or shell-quoted; nothing user-typed reaches the shell raw.
    pub fn command(&self) -> Result<String, String> {
        let cli = "\"$HOME/.cosmos/bin/cosmos-computer\"";
        let args = match self {
            Self::Click { x, y, button } => {
                let button = match button.as_deref().unwrap_or("left") {
                    b @ ("left" | "right" | "middle") => b,
                    other => return Err(format!("unknown mouse button `{other}`")),
                };
                format!("click {} {} {button}", coord(*x)?, coord(*y)?)
            }
            Self::DoubleClick { x, y } => format!("double_click {} {}", coord(*x)?, coord(*y)?),
            Self::Move { x, y } => format!("move {} {}", coord(*x)?, coord(*y)?),
            Self::Drag { x, y, to_x, to_y } => format!(
                "drag {} {} {} {}",
                coord(*x)?,
                coord(*y)?,
                coord(*to_x)?,
                coord(*to_y)?
            ),
            Self::Type { text } => {
                if text.is_empty() {
                    return Err("nothing to type".into());
                }
                format!("type {}", shell_word(text))
            }
            Self::Key { combo } => {
                let keys: Vec<&str> = combo.split_whitespace().collect();
                if keys.is_empty()
                    || !keys.iter().all(|k| {
                        k.chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '_' | '-'))
                    })
                {
                    return Err(format!("`{combo}` isn't a key combo (e.g. ctrl+l, Return)"));
                }
                format!("key {}", keys.join(" "))
            }
            Self::Scroll {
                direction,
                amount,
                x,
                y,
            } => {
                let direction = match direction.as_str() {
                    d @ ("up" | "down" | "left" | "right") => d,
                    other => return Err(format!("unknown scroll direction `{other}`")),
                };
                let amount = amount.unwrap_or(3).clamp(1, 50);
                match (x, y) {
                    (Some(x), Some(y)) => {
                        format!("scroll {direction} {amount} {} {}", coord(*x)?, coord(*y)?)
                    }
                    _ => format!("scroll {direction} {amount}"),
                }
            }
            Self::OpenUrl { url } => {
                let url = url.trim();
                if !(url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("file://"))
                {
                    return Err("only http(s):// and file:// URLs open on the seat".into());
                }
                format!("open {}", shell_word(url))
            }
        };
        Ok(format!("{} {cli} {args}", env_prefix()))
    }
}

/// Width × height from a PNG's IHDR chunk — lets the UI map a click on the
/// scaled preview back onto seat pixels without decoding the image.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh_syntax_ok(script: &str) -> bool {
        let dir = std::env::temp_dir().join(format!("cosmos-sh-{}", crate::new_id()));
        std::fs::write(&dir, script).unwrap();
        let ok = std::process::Command::new("sh")
            .arg("-n")
            .arg(&dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(true);
        let _ = std::fs::remove_file(&dir);
        ok
    }

    #[test]
    fn scripts_parse_as_posix_sh() {
        assert!(sh_syntax_ok(CLI_SCRIPT));
        assert!(sh_syntax_ok(SETUP_SCRIPT));
        assert!(sh_syntax_ok(&bootstrap_command()));
        assert!(sh_syntax_ok(&start_command()));
    }

    #[test]
    fn actions_quote_user_text() {
        let typed = SeatAction::Type {
            text: "it's $(rm -rf /)".into(),
        }
        .command()
        .unwrap();
        assert!(typed.ends_with("type 'it'\\''s $(rm -rf /)'"));
        assert!(
            SeatAction::Key {
                combo: "ctrl+l; rm".into()
            }
            .command()
            .is_err()
        );
        assert!(
            SeatAction::Click {
                x: -1,
                y: 4,
                button: None
            }
            .command()
            .is_err()
        );
        assert!(
            SeatAction::OpenUrl {
                url: "javascript:alert(1)".into()
            }
            .command()
            .is_err()
        );
        let click = SeatAction::Click {
            x: 10,
            y: 20,
            button: Some("right".into()),
        }
        .command()
        .unwrap();
        assert!(click.ends_with("click 10 20 right"));
    }

    #[test]
    fn actions_deserialize_from_camel_case() {
        let a: SeatAction =
            serde_json::from_value(serde_json::json!({"kind":"drag","x":1,"y":2,"toX":3,"toY":4}))
                .unwrap();
        assert_eq!(
            a,
            SeatAction::Drag {
                x: 1,
                y: 2,
                to_x: 3,
                to_y: 4
            }
        );
    }

    #[test]
    fn reads_png_dimensions() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&800u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1280, 800)));
        assert_eq!(png_size(b"nope"), None);
    }

    /// Real desktop end-to-end on this machine (needs Xvfb + xdotool +
    /// ImageMagick): bootstrap → wait ready → screenshot → act. Opt-in with
    /// `COSMOS_DESKTOP_E2E=1`; uses a throwaway HOME so nothing leaks.
    #[test]
    fn desktop_bootstrap_screenshot_and_act_end_to_end() {
        if std::env::var_os("COSMOS_DESKTOP_E2E").is_none() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let run = |cmd: &str| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .env("HOME", home.path())
                .output()
                .unwrap();
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        };
        let (code, out, err) = run(&start_command());
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.contains("__status=ready"), "{out}{err}");
        let shot = home.path().join("shot.png");
        let (code, out, err) = run(&format!(
            "{} cosmos-computer screenshot {}",
            env_prefix(),
            shot.display()
        ));
        assert_eq!(code, 0, "{out}{err}");
        let bytes = std::fs::read(&shot).unwrap();
        assert_eq!(png_size(&bytes), Some((1280, 800)));
        for action in [
            SeatAction::Click {
                x: 640,
                y: 400,
                button: None,
            },
            SeatAction::Type {
                text: "hello 'seat'".into(),
            },
            SeatAction::Key {
                combo: "ctrl+a Escape".into(),
            },
            SeatAction::Scroll {
                direction: "down".into(),
                amount: Some(2),
                x: Some(10),
                y: Some(10),
            },
        ] {
            let (code, out, err) = run(&action.command().unwrap());
            assert_eq!(code, 0, "{action:?}: {out}{err}");
        }
        let (_, cursor, _) = run(&format!("{} cosmos-computer cursor", env_prefix()));
        assert!(cursor.contains("x:10 y:10"), "{cursor}");
        // Recording: start, a moment of screen, stop → a finalized mp4.
        let movie = home.path().join("rec.mp4");
        let (code, out, err) = run(&format!(
            "{} cosmos-computer record start {} && sleep 2 && cosmos-computer record stop",
            env_prefix(),
            movie.display()
        ));
        assert_eq!(code, 0, "{out}{err}");
        let size = std::fs::metadata(&movie).map(|m| m.len()).unwrap_or(0);
        assert!(
            size > 1_000,
            "recording finalized ({size} bytes): {out}{err}"
        );
        // The Computer pane's capture path sees the display and its size.
        let (_, screen, _) = run(crate::cloud::LINUX_SCREEN_SCRIPT);
        let parsed = crate::cloud::parse_linux_screen(&screen).expect("screen parse");
        assert_eq!(parsed["width"], 1280);
        assert_eq!(parsed["display"], ":99");
    }

    #[test]
    fn brief_names_the_tool_and_storage() {
        let brief = agent_brief("/app/chats/abc", None);
        assert!(brief.contains("cosmos-computer screenshot"));
        assert!(brief.contains("no git remote"));
        assert!(agent_brief("/app/x", Some("o/r")).contains("checkout of o/r"));
    }
}
