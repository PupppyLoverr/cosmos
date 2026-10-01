//! Dictation: speak the task instead of typing it.
//!
//! Press Ctrl+Space (or /voice) once to start recording from the default
//! microphone (cpal → WASAPI on Windows, CoreAudio on macOS); it keeps
//! recording — the live region shows a level meter and the time — until you
//! press Ctrl+Space (or Enter) again, which stops and transcribes. Esc
//! cancels. The text lands in the composer for review; nothing is sent on
//! its own.
//!
//! Transcription, first available wins (override with `COSMOS_STT`):
//!   chatgpt  — your ChatGPT Plus/Pro sign-in (the ChatGPT apps' dictation)
//!   openai   — `gpt-4o-mini-transcribe` (falls back to `whisper-1`)
//!   groq     — `whisper-large-v3-turbo`
//!   windows  — the built-in System.Speech recogniser, offline, no key

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
#[cfg(feature = "voice")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const RATE: u32 = 16_000;
const MAX_SECS: u64 = 300;

#[derive(Clone, Debug, PartialEq)]
pub enum Backend {
    ChatGpt,
    OpenAI { key: String },
    Groq { key: String },
    Windows,
}

impl Backend {
    pub fn label(&self) -> &'static str {
        match self {
            Backend::ChatGpt => "ChatGPT",
            Backend::OpenAI { .. } => "OpenAI",
            Backend::Groq { .. } => "Groq",
            Backend::Windows => "Windows speech",
        }
    }
}

/// Pick a transcription backend from what's configured.
pub fn pick_backend() -> Option<Backend> {
    let want = zcore::env_var("STT").unwrap_or_default().to_lowercase();
    let key = |id: &str| {
        let env = zcore::catalog::provider(id).map(|p| p.env.clone()).unwrap_or_default();
        zcore::creds::key_for(id, &env).map(|(k, _)| k)
    };
    let chatgpt = || zcore::chatgpt::load().map(|_| Backend::ChatGpt);
    let openai = || key("openai").map(|key| Backend::OpenAI { key });
    let groq = || key("groq").map(|key| Backend::Groq { key });
    let windows = || cfg!(windows).then_some(Backend::Windows);
    match want.as_str() {
        "chatgpt" => chatgpt(),
        "openai" => openai(),
        "groq" => groq(),
        "windows" => windows(),
        _ => chatgpt().or_else(openai).or_else(groq).or_else(windows),
    }
}

/// A recording in progress.
pub struct Recorder {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    pub started: Instant,
    pub backend: Backend,
    rx: Receiver<Result<String>>,
    ready: Receiver<Result<()>>,
    /// the microphone is open and recording (false while it starts up)
    pub live: bool,
    created: Instant,
    /// recent levels for the meter (0..1)
    pub meter: Vec<f32>,
    pub transcribing: bool,
}

impl Recorder {
    /// Open the microphone on a background thread and return at once — the
    /// UI shows "starting" until the device is live (see `tick`).
    pub fn start(backend: Backend) -> Result<Recorder> {
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let (tx, rx) = channel();
        let (ready_tx, ready_rx) = channel::<Result<()>>();
        let (s, c, l, b) = (stop.clone(), cancel.clone(), level.clone(), backend.clone());
        std::thread::spawn(move || {
            let r = capture(&s, &c, l, &ready_tx).and_then(|pcm| {
                if c.load(Ordering::Relaxed) {
                    bail!("cancelled");
                }
                if pcm.len() < (RATE as usize) / 4 {
                    bail!("too short — hold on a little longer");
                }
                transcribe(&b, &wav(&pcm))
            });
            let _ = tx.send(r);
        });
        let now = Instant::now();
        Ok(Recorder {
            stop,
            cancel,
            level,
            started: now,
            backend,
            rx,
            ready: ready_rx,
            live: false,
            created: now,
            meter: Vec::new(),
            transcribing: false,
        })
    }

    /// Sample the current input level into the meter (call once per frame).
    pub fn tick(&mut self) {
        if !self.live {
            if let Ok(Ok(())) = self.ready.try_recv() {
                self.live = true;
                self.started = Instant::now(); // the clock starts when the mic does
            }
            return;
        }
        let v = f32::from_bits(self.level.load(Ordering::Relaxed));
        self.meter.push(v);
        if self.meter.len() > 24 {
            self.meter.remove(0);
        }
    }

    pub fn finish(&mut self) {
        self.transcribing = true;
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.stop.store(true, Ordering::Relaxed);
    }

    /// The transcript, once ready (or why there isn't one).
    pub fn poll(&self) -> Option<Result<String>> {
        if let Ok(r) = self.rx.try_recv() {
            return Some(r);
        }
        if !self.live && !self.transcribing && self.created.elapsed() > Duration::from_secs(8) {
            self.cancel();
            return Some(Err(anyhow::anyhow!("the microphone didn't start — check Windows' microphone privacy settings")));
        }
        None
    }
}

/// Built without the `voice` feature: no microphone code is linked.
#[cfg(not(feature = "voice"))]
fn capture(_stop: &AtomicBool, _cancel: &AtomicBool, _level: Arc<AtomicU32>, ready: &std::sync::mpsc::Sender<Result<()>>) -> Result<Vec<i16>> {
    let _ = ready.send(Err(anyhow::anyhow!("this cosmostui build has no microphone support (rebuild with --features voice)")));
    bail!("this cosmostui build has no microphone support (rebuild with --features voice)")
}

/// Record until `stop`; returns 16 kHz mono i16 PCM.
#[cfg(feature = "voice")]
fn capture(stop: &AtomicBool, cancel: &AtomicBool, level: Arc<AtomicU32>, ready: &std::sync::mpsc::Sender<Result<()>>) -> Result<Vec<i16>> {
    let host = cpal::default_host();
    let dev = match host.default_input_device() {
        Some(d) => d,
        None => {
            let _ = ready.send(Err(anyhow::anyhow!("no microphone found")));
            bail!("no microphone found");
        }
    };
    let supported = match dev.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(anyhow::anyhow!("microphone unavailable: {e}")));
            bail!("microphone unavailable: {e}");
        }
    };
    let fmt = supported.sample_format();
    let config = supported.config();
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate;
    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(rate as usize * 30)));
    let max = rate as usize * MAX_SECS as usize;

    // downmix to mono + track RMS for the meter
    let push = {
        let buf = buf.clone();
        let level = level.clone();
        move |frames: &mut dyn Iterator<Item = f32>| {
            let mut b = buf.lock().unwrap();
            let (mut sum, mut n, mut acc, mut k) = (0f32, 0usize, 0f32, 0usize);
            for s in frames {
                acc += s;
                k += 1;
                if k == channels {
                    let m = acc / channels as f32;
                    if b.len() < max {
                        b.push(m);
                    }
                    sum += m * m;
                    n += 1;
                    acc = 0.0;
                    k = 0;
                }
            }
            if n > 0 {
                let rms = (sum / n as f32).sqrt();
                level.store((rms * 6.0).min(1.0).to_bits(), Ordering::Relaxed);
            }
        }
    };
    // stream errors mid-recording just end the clip early; never print over the TUI
    let err = |_e: cpal::Error| {};
    let stream = match fmt {
        cpal::SampleFormat::F32 => {
            let p = push.clone();
            dev.build_input_stream(config, move |d: &[f32], _: &_| p(&mut d.iter().copied()), err, None)
        }
        cpal::SampleFormat::I16 => {
            let p = push.clone();
            dev.build_input_stream(config, move |d: &[i16], _: &_| p(&mut d.iter().map(|s| *s as f32 / 32768.0)), err, None)
        }
        cpal::SampleFormat::I32 => {
            let p = push.clone();
            dev.build_input_stream(config, move |d: &[i32], _: &_| p(&mut d.iter().map(|s| *s as f32 / 2_147_483_648.0)), err, None)
        }
        cpal::SampleFormat::U16 => {
            let p = push.clone();
            dev.build_input_stream(config, move |d: &[u16], _: &_| p(&mut d.iter().map(|s| (*s as f32 - 32768.0) / 32768.0)), err, None)
        }
        other => {
            let _ = ready.send(Err(anyhow::anyhow!("unsupported microphone format {other:?}")));
            bail!("unsupported microphone format {other:?}");
        }
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(anyhow::anyhow!("couldn't open the microphone: {e}")));
            bail!("couldn't open the microphone: {e}");
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready.send(Err(anyhow::anyhow!("couldn't start the microphone: {e}")));
        bail!("couldn't start the microphone: {e}");
    }
    let _ = ready.send(Ok(()));
    let t0 = Instant::now();
    while !stop.load(Ordering::Relaxed) && t0.elapsed() < Duration::from_secs(MAX_SECS) {
        std::thread::sleep(Duration::from_millis(30));
    }
    drop(stream);
    if cancel.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }
    let mono = std::mem::take(&mut *buf.lock().unwrap());
    Ok(resample(&mono, rate, RATE))
}

/// Linear-interpolation resample to `to` Hz, as i16.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<i16> {
    if x.is_empty() || from == 0 {
        return Vec::new();
    }
    let n = (x.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let j = p.floor() as usize;
            let f = (p - j as f64) as f32;
            let a = x[j.min(x.len() - 1)];
            let b = x[(j + 1).min(x.len() - 1)];
            ((a + (b - a) * f).clamp(-1.0, 1.0) * 32767.0) as i16
        })
        .collect()
}

/// 16-bit mono PCM WAV at `RATE`.
pub fn wav(pcm: &[i16]) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

fn transcribe(b: &Backend, wav: &[u8]) -> Result<String> {
    let allowed = zcore::config::Config::load(&zcore::cli_home()).allowed_hosts();
    let text = match b {
        Backend::ChatGpt => {
            let t = zcore::chatgpt::fresh().context("not signed in to ChatGPT — /login, or set COSMOS_STT")?;
            zproviders::transcribe_chatgpt("https://chatgpt.com/backend-api", &t.access, &t.account_id, wav, &allowed)?
        }
        Backend::OpenAI { key } => zproviders::transcribe("https://api.openai.com/v1", key, "gpt-4o-mini-transcribe", wav, &allowed)
            .or_else(|_| zproviders::transcribe("https://api.openai.com/v1", key, "whisper-1", wav, &allowed))?,
        Backend::Groq { key } => zproviders::transcribe("https://api.groq.com/openai/v1", key, "whisper-large-v3-turbo", wav, &allowed)?,
        Backend::Windows => windows_speech(wav)?,
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        bail!("didn't catch that — try again a little closer to the mic");
    }
    Ok(text)
}

/// Offline recognition with the Windows speech engine via PowerShell.
fn windows_speech(wav: &[u8]) -> Result<String> {
    let path = std::env::temp_dir().join(format!("cosmos-dictation-{}.wav", std::process::id()));
    std::fs::write(&path, wav)?;
    let script = format!(
        "$ErrorActionPreference='Stop'; Add-Type -AssemblyName System.Speech; \
         $r = New-Object System.Speech.Recognition.SpeechRecognitionEngine; \
         $r.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar)); \
         $r.SetInputToWaveFile('{}'); $o = @(); \
         try {{ while ($true) {{ $x = $r.Recognize(); if ($x -eq $null) {{ break }}; $o += $x.Text }} }} \
         catch {{ if ($_.Exception.Message -notmatch 'audio input') {{ throw }} }}; \
         $r.Dispose(); [Console]::Out.Write(($o -join ' '))",
        path.display().to_string().replace('\'', "''")
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("couldn't run PowerShell for offline dictation")?;
    let _ = std::fs::remove_file(&path);
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("Windows speech failed: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_valid() {
        let w = wav(&[0, 1000, -1000]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), RATE);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 44 + 6);
    }

    #[test]
    fn resample_48k_to_16k() {
        let x: Vec<f32> = (0..48_000).map(|i| (i as f32 / 48.0).sin() * 0.5).collect();
        let y = resample(&x, 48_000, 16_000);
        assert_eq!(y.len(), 16_000);
        assert!(y.iter().any(|s| s.abs() > 10_000));
    }
}

/// Hardware check (needs a microphone): `cargo test -p ztui mic_smoke -- --ignored --nocapture`
#[cfg(test)]
mod hardware {
    use super::*;

    /// Speak a sentence with Windows TTS into a 16 kHz mono WAV, then recognise it.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn windows_speech_round_trip() {
        let path = std::env::temp_dir().join("cosmos-tts-check.wav");
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             $f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono); \
             $s.SetOutputToWaveFile('{}', $f); $s.Speak('please fix the failing test in the parser'); $s.Dispose()",
            path.display()
        );
        let st = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status().unwrap();
        assert!(st.success());
        let wav = std::fs::read(&path).unwrap();
        let text = windows_speech(&wav).unwrap().to_lowercase();
        println!("recognised: {text:?}");
        assert!(text.contains("test") || text.contains("parser") || text.contains("fix"), "{text}");
    }

    #[test]
    #[ignore]
    fn chatgpt_transcribes_speech() {
        // needs a ChatGPT sign-in: speak with Windows TTS, transcribe via ChatGPT
        let path = std::env::temp_dir().join("cosmos-tts-chatgpt.wav");
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             $f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono); \
             $s.SetOutputToWaveFile('{}', $f); $s.Speak('please fix the failing test in the parser'); $s.Dispose()",
            path.display()
        );
        assert!(std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status().unwrap().success());
        let text = transcribe(&Backend::ChatGpt, &std::fs::read(&path).unwrap()).unwrap().to_lowercase();
        println!("recognised: {text:?}");
        assert!(text.contains("failing test") && text.contains("parser"), "{text}");
    }

    #[test]
    #[ignore]
    fn mic_smoke() {
        let mut r = match Recorder::start(Backend::Windows) {
            Ok(r) => r,
            Err(e) => panic!("mic failed to start: {e:#}"),
        };
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(50));
            r.tick();
        }
        assert!(r.live, "microphone never went live");
        println!("meter: {:?}", r.meter.iter().map(|v| (v * 100.0).round() as i32).collect::<Vec<_>>());
        r.finish();
        let t0 = Instant::now();
        loop {
            if let Some(res) = r.poll() {
                println!("result after {:?}: {res:?}", t0.elapsed());
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(60), "no result");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
