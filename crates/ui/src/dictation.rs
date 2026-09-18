//! Push-to-dictate for the composer.
//!
//! Windows runs the OS recognizer — WinRT `SpeechRecognizer` in continuous
//! dictation mode, the engine behind the system's voice typing — on a
//! dedicated thread, streaming live hypotheses and committed phrases back
//! over a channel. The level meter samples the default capture endpoint's
//! WASAPI peak (`IAudioMeterInformation`, the same meter Windows Settings
//! shows), so the visualizer tracks real input without a second capture
//! stream.
//!
//! Other platforms report [`Dictation::supported`] = false and the composer
//! hides the mic affordance.

use std::sync::mpsc;
use std::thread::JoinHandle;

use futures::channel::mpsc::UnboundedReceiver;

/// One message from the dictation worker to the composer.
#[derive(Debug)]
pub enum DictationEvent {
    /// Startup finished; `metered` reports whether real input levels will
    /// stream in (otherwise the visualizer shows an idle sway).
    Ready { metered: bool },
    /// A live partial phrase — replaces the previous hypothesis in place.
    Hypothesis(String),
    /// A finalized phrase — becomes permanent draft text.
    Committed(String),
    /// Mic peak level 0.0–1.0.
    Level(f32),
    /// The recognizer ended on its own (auto-stop timeout, device lost).
    Ended,
    /// Startup or runtime failure with the underlying OS error text — the
    /// raw message is what distinguishes a privacy toggle from a missing
    /// speech pack or a dead mic.
    Failed(String),
}

/// A live dictation session. [`Dictation::stop`] — or just dropping it —
/// ends capture; the worker unwinds the recognizer on its own thread so the
/// UI never waits on the in-flight utterance draining.
pub struct Dictation {
    stop: mpsc::Sender<()>,
    /// Kept so the OS thread stays owned by the session; its Drop detaches.
    #[allow(dead_code)]
    thread: JoinHandle<()>,
}

impl Dictation {
    /// Whether this build has a platform dictation backend.
    pub fn supported() -> bool {
        cfg!(windows)
    }

    /// Start a dictation session; events arrive on the returned receiver.
    pub fn start() -> Result<(Self, UnboundedReceiver<DictationEvent>), String> {
        #[cfg(windows)]
        {
            windows_impl::start()
        }
        #[cfg(not(windows))]
        {
            Err("Dictation isn't available on this platform yet".to_string())
        }
    }

    /// End the session: signal the worker and detach it. Joining here would
    /// stall the UI while the recognizer drains the in-flight utterance.
    pub fn stop(self) {
        let _ = self.stop.send(());
        // `thread` detaches on drop — the worker unwinds itself.
    }

    /// Run the recognizer startup path in the calling process and report the
    /// outcome — the `ZERON_DICTATION_PROBE` escape hatch that answers "does
    /// `StartAsync` fail here too?" without the app's runtime state around it.
    #[cfg(windows)]
    pub fn probe(mode: &str) -> Result<String, String> {
        windows_impl::probe(mode)
    }
}

impl Drop for Dictation {
    fn drop(&mut self) {
        // Dropping the sender disconnects the worker's `recv_timeout`,
        // which it treats as the same stop signal.
        let _ = self.stop.send(());
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{Dictation, DictationEvent};
    use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Foundation::{TimeSpan, TypedEventHandler};
    use windows::Media::SpeechRecognition::{
        SpeechContinuousRecognitionCompletedEventArgs,
        SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
        SpeechRecognitionHypothesisGeneratedEventArgs, SpeechRecognitionResultStatus,
        SpeechRecognitionScenario, SpeechRecognitionTopicConstraint, SpeechRecognizer,
        SpeechRecognizerState,
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{
        IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
    use windows::core::{h, w};

    /// Visualizer sample cadence — 25fps of levels is plenty for bars.
    const LEVEL_POLL_MS: u64 = 40;

    /// The recognizer runs on a dedicated thread — the failure event carries
    /// the OS error text, so keep the HRESULT alongside the message: that
    /// pair is what separates a privacy toggle from a missing speech pack.
    fn err_text(e: &windows::core::Error) -> String {
        let code = e.code().0 as u32;
        let message = e.message();
        if code == 0 {
            message
        } else {
            format!("{message} (0x{code:08X})")
        }
    }

    fn seconds(secs: u64) -> TimeSpan {
        TimeSpan {
            Duration: (secs * 10_000_000) as i64,
        }
    }

    fn millis(ms: u64) -> TimeSpan {
        TimeSpan {
            Duration: (ms * 10_000) as i64,
        }
    }

    pub fn start() -> Result<(Dictation, UnboundedReceiver<DictationEvent>), String> {
        let (events_tx, events_rx) = futures::channel::mpsc::unbounded();
        let (stop_tx, stop_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("zeron-dictation".into())
            .spawn(move || run(stop_rx, events_tx))
            .map_err(|e| format!("Could not start dictation ({e})"))?;
        Ok((
            Dictation {
                stop: stop_tx,
                thread,
            },
            events_rx,
        ))
    }

    struct Running {
        recognizer: SpeechRecognizer,
        session: SpeechContinuousRecognitionSession,
        meter: Option<IAudioMeterInformation>,
    }

    fn run(stop_rx: mpsc::Receiver<()>, events: UnboundedSender<DictationEvent>) {
        // The recognizer and its handlers live on this thread's apartment.
        // RoInitialize (not just CoInitializeEx) — WinRT classes want the
        // full runtime initialized, and a bare CoInit can surface as a
        // StartAsync failure on some builds.
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        match start_session(&events) {
            Ok(Running {
                recognizer,
                session,
                meter,
            }) => {
                let _ = events.unbounded_send(DictationEvent::Ready {
                    metered: meter.is_some(),
                });
                let mut meter = meter;
                loop {
                    match stop_rx.recv_timeout(Duration::from_millis(LEVEL_POLL_MS)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if let Some(active) = meter.as_ref() {
                                match unsafe { active.GetPeakValue() } {
                                    Ok(peak) => {
                                        let _ = events.unbounded_send(DictationEvent::Level(
                                            peak.clamp(0.0, 1.0),
                                        ));
                                    }
                                    // The meter died mid-session — stop
                                    // sending levels; the visualizer falls
                                    // back to its idle sway.
                                    Err(_) => meter = None,
                                }
                            }
                        }
                        // Ok(()) = explicit stop; Err(Disconnected) = the
                        // handle was dropped.
                        _ => break,
                    }
                }
                // A graceful stop drains the in-flight utterance into a
                // final ResultGenerated before the session lets go.
                let _ = session.StopAsync().and_then(|pending| pending.get());
                let _ = recognizer.Close();
            }
            Err(reason) => {
                let _ = events.unbounded_send(DictationEvent::Failed(reason));
            }
        }
        let _ = events.unbounded_send(DictationEvent::Ended);
    }

    fn start_session(events: &UnboundedSender<DictationEvent>) -> Result<Running, String> {
        let recognizer = SpeechRecognizer::new().map_err(|e| {
            format!(
                "Speech recognition isn't available on this device ({})",
                err_text(&e)
            )
        })?;

        // Free dictation, not a grammar list.
        let topic = SpeechRecognitionTopicConstraint::Create(
            SpeechRecognitionScenario::Dictation,
            h!("dictation"),
        )
        .map_err(|e| {
            format!(
                "Speech recognition isn't available on this device ({})",
                err_text(&e)
            )
        })?;
        recognizer
            .Constraints()
            .and_then(|constraints| constraints.Append(&topic))
            .map_err(|e| format!("Couldn't apply the dictation grammar ({})", err_text(&e)))?;
        let compiled = recognizer
            .CompileConstraintsAsync()
            .and_then(|pending| pending.get())
            .map_err(|e| format!("Speech recognition couldn't start ({})", err_text(&e)))?;
        let status = compiled
            .Status()
            .map_err(|e| format!("Speech recognition couldn't start ({})", err_text(&e)))?;
        if status != SpeechRecognitionResultStatus::Success {
            return Err(match status {
                SpeechRecognitionResultStatus::TopicLanguageNotSupported => {
                    "Speech recognition isn't installed for your language".to_string()
                }
                SpeechRecognitionResultStatus::MicrophoneUnavailable => {
                    "No microphone is available".to_string()
                }
                _ => format!("Speech recognition couldn't start ({status:?})"),
            });
        }

        if let Ok(timeouts) = recognizer.Timeouts() {
            // Generous floor for thinking pauses; a short end-of-phrase
            // timeout so finalized text lands quickly.
            let _ = timeouts.SetInitialSilenceTimeout(seconds(30));
            let _ = timeouts.SetEndSilenceTimeout(millis(1500));
            let _ = timeouts.SetBabbleTimeout(seconds(60));
        }

        let hypothesis_tx = events.clone();
        recognizer
            .HypothesisGenerated(&TypedEventHandler::<
                SpeechRecognizer,
                SpeechRecognitionHypothesisGeneratedEventArgs,
            >::new(move |_sender, args| {
                if let Ok(args) = args.ok()
                    && let Ok(hypothesis) = args.Hypothesis()
                    && let Ok(text) = hypothesis.Text()
                {
                    let text = text.to_string_lossy();
                    if !text.trim().is_empty() {
                        let _ = hypothesis_tx.unbounded_send(DictationEvent::Hypothesis(text));
                    }
                }
                Ok(())
            }))
            .map_err(|e| format!("Speech recognition couldn't start ({})", err_text(&e)))?;

        let session = recognizer
            .ContinuousRecognitionSession()
            .map_err(|e| format!("Speech recognition couldn't start ({})", err_text(&e)))?;
        // Silence shouldn't end a dictation session — the mic button does.
        let _ = session.SetAutoStopSilenceTimeout(seconds(600));

        let committed_tx = events.clone();
        session
            .ResultGenerated(&TypedEventHandler::<
                SpeechContinuousRecognitionSession,
                SpeechContinuousRecognitionResultGeneratedEventArgs,
            >::new(move |_sender, args| {
                if let Ok(args) = args.ok()
                    && let Ok(result) = args.Result()
                    && result.Status() == Ok(SpeechRecognitionResultStatus::Success)
                    && let Ok(text) = result.Text()
                {
                    let text = text.to_string_lossy();
                    if !text.trim().is_empty() {
                        let _ = committed_tx.unbounded_send(DictationEvent::Committed(text));
                    }
                }
                Ok(())
            }))
            .map_err(|e| format!("Speech recognition couldn't start ({})", err_text(&e)))?;

        let ended_tx = events.clone();
        session
            .Completed(&TypedEventHandler::<
                SpeechContinuousRecognitionSession,
                SpeechContinuousRecognitionCompletedEventArgs,
            >::new(move |_sender, args| {
                // A completed session isn't always a clean stop — a lost mic
                // or a speech-service timeout ends it too, and the status is
                // the only way to tell them apart.
                if let Ok(args) = args.ok()
                    && let Ok(status) = args.Status()
                    && status != SpeechRecognitionResultStatus::Success
                    && status != SpeechRecognitionResultStatus::UserCanceled
                {
                    let _ = ended_tx.unbounded_send(DictationEvent::Failed(match status {
                        SpeechRecognitionResultStatus::MicrophoneUnavailable => {
                            "Microphone became unavailable".to_string()
                        }
                        SpeechRecognitionResultStatus::TimeoutExceeded => {
                            "Dictation timed out".to_string()
                        }
                        SpeechRecognitionResultStatus::PauseLimitExceeded => {
                            "Dictation paused too long".to_string()
                        }
                        _ => format!("Dictation stopped ({status:?})"),
                    }));
                }
                let _ = ended_tx.unbounded_send(DictationEvent::Ended);
                Ok(())
            }))
            .map_err(|e| format!("Speech recognition couldn't start ({})", err_text(&e)))?;

        // Non-fatal: without the endpoint meter the bars idle-sway instead.
        let meter = endpoint_meter();

        // StartAsync throws a bare InvalidOperation (0x80131509) when the
        // shared engine isn't Idle — a stale session from a crashed attempt
        // or another speech client — so read the state and name it, and give
        // a just-settling engine one retry before surfacing the error.
        if let Ok(state) = recognizer.State()
            && state != SpeechRecognizerState::Idle
        {
            return Err(format!(
                "Speech recognizer is busy ({state:?}) — close voice typing or other dictation apps, then retry"
            ));
        }
        let started = session
            .StartAsync()
            .and_then(|pending| pending.get())
            .or_else(|_| {
                std::thread::sleep(Duration::from_millis(400));
                session.StartAsync().and_then(|pending| pending.get())
            });
        started.map_err(|e| {
            format!(
                "Could not start dictation — {} — {}",
                err_text(&e),
                consent_hint()
            )
        })?;

        Ok(Running {
            recognizer,
            session,
            meter,
        })
    }

    /// The startup path run synchronously — identical to what the worker
    /// thread does, callable from a test or the app itself for diagnosis.
    ///
    /// `mode` selects the calling context: `"ole"` first puts the calling
    /// thread in an STA (what gpui's `OleInitialize` does on the main
    /// thread) and runs the session on a spawned MTA worker — the exact
    /// apartment mix the app uses; `"aumid"` stamps an AppUserModelID like
    /// gpui's platform init does; anything else runs it inline.
    pub fn probe(mode: &str) -> Result<String, String> {
        if mode == "ole" || mode == "aumid" {
            unsafe {
                use windows::Win32::System::Ole::OleInitialize;
                let _ = OleInitialize(None);
                if mode == "aumid" {
                    let id = windows::core::HSTRING::from("com.cosmos.app");
                    let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(
                        windows::core::PCWSTR(id.as_ptr()),
                    );
                }
            }
            let join = std::thread::spawn(probe_worker);
            return join
                .join()
                .unwrap_or_else(|_| Err("probe thread panicked".into()));
        }
        probe_worker()
    }

    fn probe_worker() -> Result<String, String> {
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let running = start_session(&tx)?;
        let metered = running.meter.is_some();
        let _ = running.session.StopAsync().and_then(|p| p.get());
        Ok(format!("session started, metered={metered}"))
    }

    #[cfg(test)]
    mod tests {
        /// Manual OS-level probe: `cargo test -p zeron-ui probe -- --ignored
        /// --nocapture`. Starts the real session outside the app process —
        /// if it fails here too, the cause is Windows, not our wiring.
        #[test]
        #[ignore = "needs a live mic and speech service"]
        fn probe_start_session() {
            super::probe("inline").unwrap();
        }
    }

    /// The two consent gates WinRT dictation sits behind — read only on a
    /// startup failure so the error names the exact Settings toggle instead
    /// of guessing.
    fn consent_hint() -> &'static str {
        unsafe {
            // Online speech recognition — the WinRT recognizer's master
            // switch; absent means untouched (on by default).
            let mut accepted = 1u32;
            let mut len = 4u32;
            if RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Speech_OneCore\Settings\OnlineSpeechPrivacy"),
                w!("HasAccepted"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut accepted as *mut u32 as *mut _),
                Some(&mut len),
            )
            .is_ok()
                && accepted == 0
            {
                return "turn on Settings → Privacy & security → Speech → Online speech recognition";
            }

            // Microphone access for the device class — only an explicit
            // "Deny" is a block.
            let mut value = [0u16; 8];
            let mut len = 16u32;
            if RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\CurrentVersion\DeviceAccess\Global\{E5323777-F976-4f5b-9B55-B94699C46E44}"),
                w!("Value"),
                RRF_RT_REG_SZ,
                None,
                Some(value.as_mut_ptr() as *mut _),
                Some(&mut len),
            )
            .is_ok()
                && len >= 8
                && value[..4] == [b'D' as u16, b'e' as u16, b'n' as u16, b'y' as u16]
            {
                return "turn on Settings → Privacy & security → Microphone";
            }
        }
        "check microphone access and Windows speech settings"
    }

    /// The default capture endpoint's peak meter — the same source the
    /// Settings microphone test reads — live while the recognizer streams.
    fn endpoint_meter() -> Option<IAudioMeterInformation> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .ok()?;
            device
                .Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
                .ok()
        }
    }
}
