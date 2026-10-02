//! Getting your attention when a turn finishes, needs you, or fails: the
//! terminal title (OSC 0), the bell, a desktop notification, and an
//! optional chime. Focus-aware — with `"unfocused"` (the default) nothing
//! interrupts while you're looking at the terminal. Terminals that never
//! report focus fall back to "the turn took at least `min_secs`".
//!
//! No new dependencies: the desktop notification shells out to what the OS
//! already has (a WinRT toast through PowerShell, `osascript`,
//! `notify-send`), detached so the UI never waits on it.

use crate::skin::NotifyCfg;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Done,
    Waiting,
    Error,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Done => "done",
            Kind::Waiting => "waiting",
            Kind::Error => "error",
        }
    }
}

/// Whether an alert in `mode` ("unfocused" / "always" / "never") fires.
/// `focused` is `None` when the terminal has never reported focus.
pub fn wanted(mode: &str, focused: Option<bool>, secs: u64, min_secs: u64, kind: Kind) -> bool {
    let long = secs >= min_secs;
    if mode == "never" || (kind != Kind::Waiting && !long) {
        return false;
    }
    match (mode, focused) {
        ("always", _) => true,
        (_, Some(f)) => !f,
        (_, None) => long,
    }
}

/// What to do for one moment: bytes for the terminal, plus side effects.
#[derive(Debug, Default, PartialEq)]
pub struct Alert {
    /// title / bell escapes, written between frames
    pub term: Vec<u8>,
    pub desktop: Option<(String, String)>,
    pub chime: bool,
}

/// OSC 0: set the window title (control characters dropped).
pub fn title(text: &str) -> Vec<u8> {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]0;{clean}\x07").into_bytes()
}

/// Decide the alert for `kind` after a turn of `secs` seconds.
pub fn alert(cfg: &NotifyCfg, kind: Kind, focused: Option<bool>, secs: u64, head: &str, body: &str) -> Alert {
    let mut a = Alert::default();
    if !cfg.events.iter().any(|e| e == kind.key()) {
        return a;
    }
    if cfg.title {
        let mark = match kind {
            Kind::Done => "✓",
            Kind::Waiting => "⚠",
            Kind::Error => "✖",
        };
        a.term.extend(title(&format!("{mark} {head} · cosmostui")));
    }
    let ring = wanted(&cfg.bell, focused, secs, cfg.min_secs, kind);
    if wanted(&cfg.desktop, focused, secs, cfg.min_secs, kind) {
        a.desktop = Some((format!("cosmostui — {head}"), body.to_string()));
    }
    if ring && cfg.sound && chime_available() {
        a.chime = true;
    } else if ring {
        a.term.push(0x07);
    }
    a
}

/// Run the side effects (never blocks).
pub fn fire(a: &Alert) {
    if let Some((t, b)) = &a.desktop {
        desktop(t, b);
    }
    if a.chime {
        chime();
    }
}

/// Spawn and reap in the background; failures are silent.
fn spawn(mut cmd: std::process::Command) {
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    std::thread::spawn(move || {
        let _ = cmd.status();
    });
}

/// Standard base64 (for PowerShell's -EncodedCommand).
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The PowerShell that shows a WinRT toast (as PowerShell's own app id,
/// which every Windows 10/11 machine has registered).
fn toast_script(head: &str, body: &str) -> String {
    let q = |s: &str| s.replace('\'', "''");
    format!(
        "$ErrorActionPreference='SilentlyContinue';\
         [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null;\
         $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);\
         $t = $x.GetElementsByTagName('text');\
         $t.Item(0).AppendChild($x.CreateTextNode('{}')) | Out-Null;\
         $t.Item(1).AppendChild($x.CreateTextNode('{}')) | Out-Null;\
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($x))",
        q(head),
        q(body)
    )
}

fn desktop(head: &str, body: &str) {
    if cfg!(windows) {
        let utf16: Vec<u8> = toast_script(head, body).encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-EncodedCommand", &base64(&utf16)]);
        spawn(c);
    } else if cfg!(target_os = "macos") {
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let mut c = std::process::Command::new("osascript");
        c.args(["-e", &format!("display notification \"{}\" with title \"{}\"", q(body), q(head))]);
        spawn(c);
    } else {
        let mut c = std::process::Command::new("notify-send");
        c.args(["--app-name=cosmostui", head, body]);
        spawn(c);
    }
}

fn chime_available() -> bool {
    cfg!(feature = "voice")
}

/// Two soft rising notes (~0.35 s) on the default output device.
#[cfg(feature = "voice")]
fn chime() {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    std::thread::spawn(|| {
        let host = cpal::default_host();
        let Some(dev) = host.default_output_device() else { return };
        let Ok(sup) = dev.default_output_config() else { return };
        let fmt = sup.sample_format();
        let config = sup.config();
        let ch = config.channels.max(1) as usize;
        let rate = config.sample_rate as f32;
        let tone = move |n: usize| -> f32 {
            let t = n as f32 / rate;
            let (f, t0) = if t < 0.16 { (880.0, 0.0) } else { (1318.5, 0.16) };
            if t > 0.36 {
                return 0.0;
            }
            let local = t - t0;
            let env = (local / 0.005).min(1.0) * (-local * 14.0).exp();
            0.18 * env * (std::f32::consts::TAU * f * t).sin()
        };
        let err = |_e: cpal::Error| {};
        let mut n = 0usize;
        let stream = match fmt {
            cpal::SampleFormat::F32 => dev.build_output_stream(
                config,
                move |d: &mut [f32], _: &_| {
                    for fr in d.chunks_mut(ch) {
                        let v = tone(n);
                        n += 1;
                        fr.iter_mut().for_each(|s| *s = v);
                    }
                },
                err,
                None,
            ),
            cpal::SampleFormat::I16 => dev.build_output_stream(
                config,
                move |d: &mut [i16], _: &_| {
                    for fr in d.chunks_mut(ch) {
                        let v = (tone(n) * 32767.0) as i16;
                        n += 1;
                        fr.iter_mut().for_each(|s| *s = v);
                    }
                },
                err,
                None,
            ),
            cpal::SampleFormat::U16 => dev.build_output_stream(
                config,
                move |d: &mut [u16], _: &_| {
                    for fr in d.chunks_mut(ch) {
                        let v = (tone(n) * 32767.0 + 32768.0) as u16;
                        n += 1;
                        fr.iter_mut().for_each(|s| *s = v);
                    }
                },
                err,
                None,
            ),
            _ => return,
        };
        if let Ok(s) = stream {
            if s.play().is_ok() {
                std::thread::sleep(std::time::Duration::from_millis(450));
            }
        }
    });
}

#[cfg(not(feature = "voice"))]
fn chime() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_interrupts_when_you_look_away() {
        // focus known: unfocused only
        assert!(wanted("unfocused", Some(false), 30, 15, Kind::Done));
        assert!(!wanted("unfocused", Some(true), 30, 15, Kind::Done));
        // short turns never notify; a permission prompt always may
        assert!(!wanted("always", Some(false), 3, 15, Kind::Done));
        assert!(wanted("unfocused", Some(false), 1, 15, Kind::Waiting));
        // focus unknown: fall back to "it took a while"
        assert!(wanted("unfocused", None, 40, 15, Kind::Done));
        assert!(!wanted("unfocused", None, 5, 15, Kind::Waiting));
        assert!(!wanted("never", Some(false), 99, 15, Kind::Error));
        assert!(wanted("always", Some(true), 99, 15, Kind::Error));
    }

    #[test]
    fn alerts_are_built_from_the_config() {
        let cfg = NotifyCfg::default();
        let a = alert(&cfg, Kind::Done, Some(false), 60, "Done in 1m", "fixed the bug");
        let s = String::from_utf8_lossy(&a.term);
        assert!(s.starts_with("\x1b]0;✓ Done in 1m · cosmostui\x07"), "{s:?}");
        assert!(s.ends_with('\x07') && a.term.iter().filter(|b| **b == 7).count() == 2, "title + bell: {s:?}");
        assert_eq!(a.desktop, Some(("cosmostui — Done in 1m".into(), "fixed the bug".into())));
        // looking at it: title only
        let a = alert(&cfg, Kind::Done, Some(true), 60, "Done", "");
        assert_eq!(a.term, title("✓ Done · cosmostui"));
        assert!(a.desktop.is_none());
        // events filter
        let mut quiet = cfg.clone();
        quiet.events = vec!["waiting".into()];
        assert_eq!(alert(&quiet, Kind::Done, Some(false), 60, "Done", ""), Alert::default());
        assert_eq!(title("a\x1bb\nc"), b"\x1b]0;abc\x07".to_vec());
    }

    /// Shows a real notification — `cargo test -p ztui toast_smoke -- --ignored`.
    #[test]
    #[ignore]
    fn toast_smoke() {
        let utf16: Vec<u8> = toast_script("cosmostui — smoke test", "If you can read this, toasts work.").encode_utf16().flat_map(u16::to_le_bytes).collect();
        if cfg!(windows) {
            let out = std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &base64(&utf16)])
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        }
    }

    #[test]
    fn base64_and_toast_quoting() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        let s = toast_script("it's done", "a'b");
        assert!(s.contains("CreateTextNode('it''s done')") && s.contains("CreateTextNode('a''b')"));
        assert!(s.contains("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell"));
    }
}
