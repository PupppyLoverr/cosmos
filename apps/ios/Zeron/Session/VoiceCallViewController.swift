import AVFoundation
import UIKit

@MainActor
final class VoiceCallViewController: UIViewController, AVSpeechSynthesizerDelegate {
    private let source: SessionSource
    private let latestReply: () -> String?
    private let initialTitle: String
    private var modelLabel: String
    private let capture = SpeechCapture(
        mode: .voiceChat,
        silenceTimeout: 1.4,
        maximumDuration: 60,
        deactivateSessionOnStop: false,
        ducksOthers: false
    )
    private let speech = AVSpeechSynthesizer()
    private let titleLabel = UILabel()
    private let model = UILabel()
    private let status = UILabel()
    private let caption = UILabel()
    private let orb = UIView()
    private let muteButton = Glass.circleButton(symbol: "mic", size: 58, pointSize: 19)
    private let endButton = Glass.circleButton(symbol: "phone.down.fill", size: 58, pointSize: 19)
    private var listenTask: Task<Void, Never>?
    private var replyTask: Task<Void, Never>?
    private var muted = false
    private var ended = false
    private var listening = false
    private var interrupted = false
    private var observedRunning = false
    private var sentRevision: UInt64 = 0
    private var sentRows: UInt32 = 0
    private var currentFrameRevision: UInt64 = 0
    private var currentFrameRows: UInt32 = 0
    private var currentChrome = SessionChrome()
    private var lastUtterance = ""
    private var previousIdleTimerState = false

    init(source: SessionSource, title: String, model: String, latestReply: @escaping () -> String?) {
        self.source = source
        self.initialTitle = title
        self.modelLabel = model
        self.latestReply = latestReply
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .overFullScreen
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        speech.delegate = self
        previousIdleTimerState = UIApplication.shared.isIdleTimerDisabled
        UIApplication.shared.isIdleTimerDisabled = true
        view.backgroundColor = UIColor.black.withAlphaComponent(0.82)

        let blur = UIVisualEffectView(effect: UIBlurEffect(style: .systemChromeMaterialDark))
        blur.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(blur)

        titleLabel.text = initialTitle
        titleLabel.textColor = .white
        titleLabel.font = Fonts.ui(.sansSemibold, 18)
        titleLabel.textAlignment = .center
        model.textColor = UIColor.white.withAlphaComponent(0.68)
        model.font = Fonts.ui(.sans, 13)
        model.textAlignment = .center
        updateModelLabel()
        status.textColor = UIColor.white.withAlphaComponent(0.8)
        status.font = Fonts.ui(.sansMedium, 16)
        status.textAlignment = .center
        caption.textColor = UIColor.white.withAlphaComponent(0.78)
        caption.font = Fonts.ui(.sans, 16)
        caption.textAlignment = .center
        caption.numberOfLines = 3
        caption.adjustsFontSizeToFitWidth = true
        caption.minimumScaleFactor = 0.8

        orb.backgroundColor = Palette.accent.withAlphaComponent(0.8)
        orb.layer.cornerRadius = 78
        orb.layer.shadowColor = Palette.accent.cgColor
        orb.layer.shadowOpacity = 0.55
        orb.layer.shadowRadius = 32
        orb.layer.shadowOffset = .zero
        orb.translatesAutoresizingMaskIntoConstraints = false
        orb.isAccessibilityElement = true
        orb.accessibilityLabel = "Voice activity"

        muteButton.accessibilityIdentifier = "voice-mute"
        muteButton.accessibilityLabel = "Mute"
        muteButton.addAction(UIAction { [weak self] _ in self?.toggleMute() }, for: .touchUpInside)
        endButton.accessibilityIdentifier = "voice-end"
        endButton.accessibilityLabel = "End voice mode"
        endButton.tintColor = Palette.danger
        endButton.addAction(UIAction { [weak self] _ in self?.endCall() }, for: .touchUpInside)

        let header = UIStackView(arrangedSubviews: [titleLabel, model])
        header.axis = .vertical
        header.alignment = .center
        header.spacing = 3
        header.translatesAutoresizingMaskIntoConstraints = false
        let controls = UIStackView(arrangedSubviews: [muteButton, endButton])
        controls.axis = .horizontal
        controls.alignment = .center
        controls.spacing = 18
        controls.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(header)
        view.addSubview(status)
        view.addSubview(orb)
        view.addSubview(caption)
        view.addSubview(controls)
        NSLayoutConstraint.activate([
            blur.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            blur.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            blur.topAnchor.constraint(equalTo: view.topAnchor),
            blur.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            header.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 20),
            header.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -24),
            status.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            status.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 28),
            orb.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            orb.centerYAnchor.constraint(equalTo: view.centerYAnchor, constant: -10),
            orb.widthAnchor.constraint(equalToConstant: 156),
            orb.heightAnchor.constraint(equalTo: orb.widthAnchor),
            caption.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 28),
            caption.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -28),
            caption.topAnchor.constraint(equalTo: orb.bottomAnchor, constant: 30),
            controls.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            controls.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -24),
        ])

        let centerTap = UITapGestureRecognizer(target: self, action: #selector(orbTapped))
        orb.addGestureRecognizer(centerTap)
        NotificationCenter.default.addObserver(self, selector: #selector(interruption(_:)), name: AVAudioSession.interruptionNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(backgrounded), name: UIApplication.didEnterBackgroundNotification, object: nil)
        startListening()
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if !ended { finishWithoutDismiss() }
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        listenTask?.cancel()
        replyTask?.cancel()
    }

    func sessionDidUpdate(chrome: SessionChrome, frame: LayoutFrame) {
        currentChrome = chrome
        currentFrameRevision = frame.revision()
        currentFrameRows = frame.rowCount()
        if !chrome.title.isEmpty { titleLabel.text = chrome.title }
        if let model = chrome.chips.first(where: { $0.id == "model" })?.title {
            modelLabel = model
            updateModelLabel()
        }
        guard !ended else { return }
        if chrome.running {
            observedRunning = true
            if listening == false, speech.isSpeaking == false { setStatus("\(modelLabel) is working…") }
        } else if observedRunning || frameChangedSinceSend {
            scheduleReplyCheck()
        }
        if case .failed = chrome.banner, (status.text?.hasSuffix("is working…") == true || status.text == "Sending…") {
            speak("Something went wrong — check the chat.")
        }
    }

    @objc private func orbTapped() {
        guard speech.isSpeaking else { return }
        speech.stopSpeaking(at: .immediate)
        caption.text = nil
        startListening()
    }

    private func startListening() {
        guard !ended, !muted, !interrupted, !listening, !speech.isSpeaking else { return }
        replyTask?.cancel()
        replyTask = nil
        lastUtterance = ""
        setStatus("Listening…")
        listenTask?.cancel()
        listenTask = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await self.capture.start(
                    onText: { [weak self] text in
                        guard let self else { return }
                        self.lastUtterance = text
                        self.caption.text = text
                        self.orb.accessibilityLabel = "Barge in"
                    },
                    onEnd: { [weak self] error in self?.listeningEnded(error) },
                    onLevel: { [weak self] level in self?.updateOrb(level: level) }
                )
                guard !self.ended, !self.muted else { return }
                self.listening = true
            } catch {
                guard !Task.isCancelled, !self.ended else { return }
                if (error as? DictationError) == .permissionDenied {
                    self.presentSpeechSettingsAlert()
                } else {
                    self.setStatus((error as? DictationError)?.message ?? "Voice isn't available right now")
                }
            }
        }
    }

    private func listeningEnded(_ error: Error?) {
        listening = false
        guard !ended, !muted, !interrupted else { return }
        if let error, lastUtterance.isEmpty {
            setStatus("Paused")
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in self?.startListening() }
            return
        }
        let text = lastUtterance.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            startListening()
            return
        }
        send(text: text)
    }

    private func send(text: String) {
        sentRevision = currentFrameRevision
        sentRows = currentFrameRows
        observedRunning = false
        lastUtterance = ""
        setStatus("Sending…")
        let mode: DeliveryMode = currentChrome.running && currentChrome.canSteer ? .steer : .queue
        guard source.send(text: text, images: [], mode: mode) else {
            setStatus("Paused")
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.startListening() }
            return
        }
    }

    private var frameChangedSinceSend: Bool {
        currentFrameRevision != sentRevision || currentFrameRows != sentRows
    }

    private func scheduleReplyCheck() {
        guard replyTask == nil, !ended else { return }
        replyTask = Task { @MainActor [weak self] in
            guard let self else { return }
            for _ in 0..<24 {
                try? await Task.sleep(nanoseconds: 200_000_000)
                guard !Task.isCancelled, !self.ended else { return }
                guard !self.currentChrome.running else { continue }
                if self.frameChangedSinceSend, let reply = self.latestReply(), !reply.isEmpty {
                    self.replyTask = nil
                    self.speak(reply)
                    return
                }
            }
            self.replyTask = nil
            self.startListening()
        }
    }

    private func speak(_ text: String) {
        guard !ended else { return }
        let cleaned = SpeechText.clean(text)
        guard !cleaned.isEmpty else { startListening(); return }
        capture.stop()
        listening = false
        caption.text = cleaned
        setStatus("Speaking…")
        orb.accessibilityLabel = "Barge in"
        let utterance = AVSpeechUtterance(string: cleaned)
        utterance.rate = 0.5
        utterance.voice = preferredVoice()
        speech.speak(utterance)
    }

    private func preferredVoice() -> AVSpeechSynthesisVoice? {
        let language = Locale.current.language.languageCode?.identifier ?? "en"
        let voices = AVSpeechSynthesisVoice.speechVoices().filter { $0.language.hasPrefix(language) }
        return voices.max { quality($0.quality) < quality($1.quality) }
            ?? AVSpeechSynthesisVoice(language: Locale.current.identifier)
            ?? AVSpeechSynthesisVoice(language: "en-US")
    }

    private func quality(_ quality: AVSpeechSynthesisVoiceQuality) -> Int {
        switch quality {
        case .premium: 3
        case .enhanced: 2
        default: 1
        }
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        guard !ended else { return }
        caption.text = nil
        startListening()
    }

    private func updateOrb(level: Float) {
        let normalized = min(1, max(0, level * 8))
        if UIAccessibility.isReduceMotionEnabled {
            orb.alpha = 0.7 + CGFloat(normalized) * 0.3
        } else {
            orb.alpha = 0.72 + CGFloat(normalized) * 0.28
            let scale = 1 + CGFloat(normalized) * 0.24
            UIView.animate(withDuration: 0.12, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction]) {
                self.orb.transform = CGAffineTransform(scaleX: scale, y: scale)
            }
        }
    }

    private func toggleMute() {
        muted.toggle()
        if muted {
            lastUtterance = ""
            capture.stop()
            speech.stopSpeaking(at: .immediate)
            listening = false
            setStatus("Paused")
            muteButton.setImage(UIImage(systemName: "mic.slash"), for: .normal)
            muteButton.accessibilityLabel = "Unmute"
        } else {
            muteButton.setImage(UIImage(systemName: "mic"), for: .normal)
            muteButton.accessibilityLabel = "Mute"
            startListening()
        }
    }

    private func setStatus(_ text: String) {
        status.text = text
        orb.accessibilityValue = text
    }

    private func updateModelLabel() {
        model.text = modelLabel
    }

    private func endCall() {
        guard !ended else { return }
        finishWithoutDismiss()
        dismiss(animated: true)
    }

    private func finishWithoutDismiss() {
        guard !ended else { return }
        ended = true
        listenTask?.cancel()
        replyTask?.cancel()
        capture.stop()
        speech.stopSpeaking(at: .immediate)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        UIApplication.shared.isIdleTimerDisabled = previousIdleTimerState
        NotificationCenter.default.removeObserver(self)
    }

    @objc private func interruption(_ note: Notification) {
        guard !ended else { return }
        guard let typeValue = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
              let type = AVAudioSession.InterruptionType(rawValue: typeValue)
        else { return }
        if type == .began {
            interrupted = true
            lastUtterance = ""
            capture.stop()
            speech.stopSpeaking(at: .immediate)
            listening = false
            setStatus("Paused")
        } else if type == .ended, !muted {
            interrupted = false
            startListening()
        }
    }

    @objc private func backgrounded() {
        endCall()
    }

    private func presentSpeechSettingsAlert() {
        let alert = UIAlertController(
            title: "Microphone or Speech Recognition access is off",
            message: nil,
            preferredStyle: .alert
        )
        alert.addAction(UIAlertAction(title: "Open Settings", style: .default) { [weak self] _ in
            self?.endCall()
            guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
            UIApplication.shared.open(url)
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel) { [weak self] _ in self?.endCall() })
        present(alert, animated: true)
    }
}
