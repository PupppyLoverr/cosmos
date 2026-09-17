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
    /// Startup or runtime failure with a short user-facing reason.
    Failed(&'static str),
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
    pub fn start() -> Result<(Self, UnboundedReceiver<DictationEvent>), &'static str> {
        #[cfg(windows)]
        {
            windows_impl::start()
        }
        #[cfg(not(windows))]
        {
            Err("Dictation isn't available on this platform yet")
        }
    }

    /// End the session: signal the worker and detach it. Joining here would
    /// stall the UI while the recognizer drains the in-flight utterance.
    pub fn stop(self) {
        let _ = self.stop.send(());
        // `thread` detaches on drop — the worker unwinds itself.
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
    };
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole};
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::core::h;

    /// Visualizer sample cadence — 25fps of levels is plenty for bars.
    const LEVEL_POLL_MS: u64 = 40;

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

    pub fn start() -> Result<(Dictation, UnboundedReceiver<DictationEvent>), &'static str> {
        let (events_tx, events_rx) = futures::channel::mpsc::unbounded();
        let (stop_tx, stop_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("zeron-dictation".into())
            .spawn(move || run(stop_rx, events_tx))
            .map_err(|_| "Could not start dictation")?;
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
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
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

    fn start_session(events: &UnboundedSender<DictationEvent>) -> Result<Running, &'static str> {
        let recognizer = SpeechRecognizer::new()
            .map_err(|_| "Speech recognition isn't available on this device")?;

        // Free dictation, not a grammar list.
        let topic = SpeechRecognitionTopicConstraint::Create(
            SpeechRecognitionScenario::Dictation,
            h!("dictation"),
        )
        .map_err(|_| "Speech recognition isn't available on this device")?;
        recognizer
            .Constraints()
            .and_then(|constraints| constraints.Append(&topic))
            .map_err(|_| "Speech recognition isn't available on this device")?;
        let compiled = recognizer
            .CompileConstraintsAsync()
            .and_then(|pending| pending.get())
            .map_err(|_| "Speech recognition couldn't start")?;
        let status = compiled
            .Status()
            .map_err(|_| "Speech recognition couldn't start")?;
        if status != SpeechRecognitionResultStatus::Success {
            return Err(match status {
                SpeechRecognitionResultStatus::TopicLanguageNotSupported => {
                    "Speech recognition isn't installed for your language"
                }
                SpeechRecognitionResultStatus::MicrophoneUnavailable => {
                    "No microphone is available"
                }
                _ => "Speech recognition couldn't start",
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
            .map_err(|_| "Speech recognition couldn't start")?;

        let session = recognizer
            .ContinuousRecognitionSession()
            .map_err(|_| "Speech recognition couldn't start")?;
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
            .map_err(|_| "Speech recognition couldn't start")?;

        let ended_tx = events.clone();
        session
            .Completed(&TypedEventHandler::<
                SpeechContinuousRecognitionSession,
                SpeechContinuousRecognitionCompletedEventArgs,
            >::new(move |_sender, _args| {
                let _ = ended_tx.unbounded_send(DictationEvent::Ended);
                Ok(())
            }))
            .map_err(|_| "Speech recognition couldn't start")?;

        // Non-fatal: without the endpoint meter the bars idle-sway instead.
        let meter = endpoint_meter();

        session
            .StartAsync()
            .and_then(|pending| pending.get())
            .map_err(|_| "Could not start dictation — check microphone access")?;

        Ok(Running {
            recognizer,
            session,
            meter,
        })
    }

    /// The default capture endpoint's peak meter — the same source the
    /// Settings microphone test reads — live while the recognizer streams.
    fn endpoint_meter() -> Option<IAudioMeterInformation> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?;
            device
                .Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
                .ok()
        }
    }
}
