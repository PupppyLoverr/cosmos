import QuartzCore
import UIKit

@MainActor
final class DotOrbView: UIView {
    enum Mode: Equatable {
        case idle
        case listening
        case thinking
        case speaking
        case paused
    }

    private struct Dot {
        let x: CGFloat
        let y: CGFloat
        let z: CGFloat
        let phase: CGFloat
        let speed: CGFloat
    }

    private struct Parameters {
        var rotation: CGFloat = 0.25
        var radius: CGFloat = 1
        var breathing: CGFloat = 0
        var ripple: CGFloat = 0
        var thinkingBand: CGFloat = 0
        var speakingBreath: CGFloat = 0
        var opacity: CGFloat = 1
        var tint = UIColor.white
    }

    private static let dots: [Dot] = {
        let count = 440
        let goldenAngle = CGFloat.pi * (3 - sqrt(5))
        return (0..<count).map { index in
            let y = 1 - 2 * (CGFloat(index) + 0.5) / CGFloat(count)
            let ring = sqrt(max(0, 1 - y * y))
            let angle = goldenAngle * CGFloat(index)
            return Dot(
                x: cos(angle) * ring,
                y: y,
                z: sin(angle) * ring,
                phase: .random(in: 0..<(2 * .pi)),
                speed: .random(in: 0.6...1.4)
            )
        }
    }()

    private let glowLayer = CAGradientLayer()
    private let buckets: [CAShapeLayer] = (0..<8).map { _ in CAShapeLayer() }
    private let highlightLayer = CAShapeLayer()
    private let displayProxy = DisplayLinkProxy()
    private var displayLink: CADisplayLink?
    private var parameters = Parameters()
    private var targetLevel: CGFloat = 0
    private var level: CGFloat = 0
    private var pulseEnergy: CGFloat = 0
    private let startedAt = CACurrentMediaTime()

    var mode: Mode = .idle {
        didSet {
            guard oldValue != mode else { return }
            if UIAccessibility.isReduceMotionEnabled {
                parameters = targetParameters(for: mode)
                renderFrame(at: CACurrentMediaTime())
            } else {
                startDisplayLinkIfNeeded()
            }
        }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        parameters = targetParameters(for: .idle)
        isAccessibilityElement = true
        accessibilityLabel = "Voice activity"

        glowLayer.type = .radial
        glowLayer.startPoint = CGPoint(x: 0.5, y: 0.5)
        glowLayer.endPoint = CGPoint(x: 1, y: 0.5)
        glowLayer.colors = [
            parameters.tint.withAlphaComponent(0.28).cgColor,
            UIColor.clear.cgColor,
        ]
        layer.addSublayer(glowLayer)

        for bucket in buckets {
            bucket.fillColor = UIColor.white.cgColor
            bucket.contentsScale = traitCollection.displayScale
            layer.addSublayer(bucket)
        }
        highlightLayer.fillColor = UIColor.white.withAlphaComponent(0.68).cgColor
        highlightLayer.contentsScale = traitCollection.displayScale
        layer.addSublayer(highlightLayer)

        displayProxy.target = self
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(reduceMotionChanged),
            name: UIAccessibility.reduceMotionStatusDidChangeNotification,
            object: nil
        )
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit {
        MainActor.assumeIsolated {
            displayLink?.invalidate()
            NotificationCenter.default.removeObserver(self)
        }
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window == nil {
            stopDisplayLink()
        } else if UIAccessibility.isReduceMotionEnabled {
            parameters = targetParameters(for: mode)
            renderFrame(at: CACurrentMediaTime())
        } else {
            startDisplayLinkIfNeeded()
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        if UIAccessibility.isReduceMotionEnabled || displayLink == nil {
            if UIAccessibility.isReduceMotionEnabled {
                parameters = targetParameters(for: mode)
            }
            renderFrame(at: CACurrentMediaTime())
        }
    }

    func setLevel(_ level: CGFloat) {
        targetLevel = min(1, max(0, level))
        if UIAccessibility.isReduceMotionEnabled {
            self.level = targetLevel
            renderFrame(at: CACurrentMediaTime())
        }
    }

    func pulse() {
        pulseEnergy = max(pulseEnergy, 1)
        if UIAccessibility.isReduceMotionEnabled {
            renderFrame(at: CACurrentMediaTime())
        }
    }

    @objc private func reduceMotionChanged() {
        if UIAccessibility.isReduceMotionEnabled {
            stopDisplayLink()
            parameters = targetParameters(for: mode)
            renderFrame(at: CACurrentMediaTime())
        } else {
            startDisplayLinkIfNeeded()
        }
    }

    fileprivate func displayTick() {
        let attackOrRelease: CGFloat = targetLevel > level ? 0.35 : 0.08
        level += (targetLevel - level) * attackOrRelease
        pulseEnergy *= 0.88

        let target = targetParameters(for: mode)
        parameters.rotation += (target.rotation - parameters.rotation) * 0.08
        parameters.radius += (target.radius - parameters.radius) * 0.08
        parameters.breathing += (target.breathing - parameters.breathing) * 0.08
        parameters.ripple += (target.ripple - parameters.ripple) * 0.08
        parameters.thinkingBand += (target.thinkingBand - parameters.thinkingBand) * 0.08
        parameters.speakingBreath += (target.speakingBreath - parameters.speakingBreath) * 0.08
        parameters.opacity += (target.opacity - parameters.opacity) * 0.08
        parameters.tint = mix(parameters.tint, target.tint, amount: 0.08)
        renderFrame(at: CACurrentMediaTime())
    }

    private func startDisplayLinkIfNeeded() {
        guard displayLink == nil, window != nil, !UIAccessibility.isReduceMotionEnabled else { return }
        let link = CADisplayLink(target: displayProxy, selector: #selector(DisplayLinkProxy.step(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    private func stopDisplayLink() {
        displayLink?.invalidate()
        displayLink = nil
    }

    private func targetParameters(for mode: Mode) -> Parameters {
        let darkTraits = UITraitCollection(userInterfaceStyle: .dark)
        let accent = Palette.accent.resolvedColor(with: darkTraits)
        switch mode {
        case .idle:
            return Parameters(rotation: 0.25, breathing: 0.02, tint: accent)
        case .listening:
            return Parameters(rotation: 0.35, ripple: 0.22, tint: accent)
        case .thinking:
            return Parameters(rotation: 1.1, radius: 0.96, thinkingBand: 1, tint: accent)
        case .speaking:
            return Parameters(
                rotation: 0.45,
                ripple: 0.22,
                speakingBreath: 0.12,
                tint: mix(accent, UIColor(hex: 0x7DD3FC), amount: 0.3)
            )
        case .paused:
            return Parameters(
                rotation: 0.12,
                radius: 0.9,
                opacity: 0.45,
                tint: Palette.tertiary.resolvedColor(with: darkTraits)
            )
        }
    }

    private func renderFrame(at timestamp: CFTimeInterval) {
        guard bounds.width > 0, bounds.height > 0 else { return }
        let time = UIAccessibility.isReduceMotionEnabled ? 0 : CGFloat(timestamp - startedAt)
        let energy = max(level, pulseEnergy)
        let radius = min(bounds.width, bounds.height) * 0.4 * parameters.radius
        let center = CGPoint(x: bounds.midX, y: bounds.midY)
        let glowSize = radius * 3
        glowLayer.bounds = CGRect(x: 0, y: 0, width: glowSize, height: glowSize)
        glowLayer.position = center
        glowLayer.colors = [
            parameters.tint.withAlphaComponent(0.28).cgColor,
            UIColor.clear.cgColor,
        ]
        let ambientEnergy: CGFloat = mode == .idle || mode == .thinking ? 0.2 : 0
        glowLayer.opacity = Float(min(1, 0.12 + max(energy, ambientEnergy) * 0.55) * parameters.opacity)

        let cosine = cos(time * parameters.rotation)
        let sine = sin(time * parameters.rotation)
        let tilt = CGFloat.pi * 18 / 180
        let tiltCosine = cos(tilt)
        let tiltSine = sin(tilt)
        let scale = window?.screen.scale ?? traitCollection.displayScale
        let bandY = sin(time * 1.6)
        let paths = (0..<buckets.count).map { _ in CGMutablePath() }
        let highlightPath = CGMutablePath()

        for dot in Self.dots {
            let rotatedX = dot.x * cosine + dot.z * sine
            let rotatedZ = -dot.x * sine + dot.z * cosine
            let rotatedY = dot.y * tiltCosine - rotatedZ * tiltSine
            let depth = dot.y * tiltSine + rotatedZ * tiltCosine
            let ripple = sin(dot.phase + time * 4 * dot.speed) * parameters.ripple * energy
            let breath = parameters.breathing * sin(time * 1.2)
                + parameters.speakingBreath * energy
            let displacement = 1 + breath + ripple
            let x = center.x + rotatedX * radius * displacement
            let y = center.y - rotatedY * radius * displacement
            let depth01 = min(1, max(0, (depth + 1) * 0.5))
            let bucket = min(buckets.count - 1, Int(depth01 * CGFloat(buckets.count)))
            let bandDistance = abs(rotatedY - bandY)
            let bandStrength = parameters.thinkingBand * max(0, 1 - bandDistance / 0.16)
            let side = (1.1 + (3 - 1.1) * depth01) * (1 + 0.4 * bandStrength)
            let pixelSide = max(1 / scale, round(side * scale) / scale)
            let pixelX = round((x - pixelSide * 0.5) * scale) / scale
            let pixelY = round((y - pixelSide * 0.5) * scale) / scale
            let rect = CGRect(x: pixelX, y: pixelY, width: pixelSide, height: pixelSide)
            paths[bucket].addRect(rect)
            if bandStrength > 0.05, depth > 0 {
                highlightPath.addRect(rect)
            }
        }

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        for (index, bucket) in buckets.enumerated() {
            let depth01 = CGFloat(index) / CGFloat(buckets.count - 1)
            let highlight: CGFloat = index == buckets.count - 1 ? 0.22 : (index == buckets.count - 2 ? 0.1 : 0)
            bucket.fillColor = mix(parameters.tint, .white, amount: highlight).cgColor
            bucket.opacity = Float((0.12 + 0.88 * depth01) * parameters.opacity)
            bucket.path = paths[index]
        }
        highlightLayer.path = highlightPath
        highlightLayer.opacity = Float(parameters.thinkingBand * parameters.opacity)
        CATransaction.commit()
    }

    private func mix(_ first: UIColor, _ second: UIColor, amount: CGFloat) -> UIColor {
        var r1: CGFloat = 0
        var g1: CGFloat = 0
        var b1: CGFloat = 0
        var a1: CGFloat = 0
        var r2: CGFloat = 0
        var g2: CGFloat = 0
        var b2: CGFloat = 0
        var a2: CGFloat = 0
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
        let start = first.resolvedColor(with: UITraitCollection(userInterfaceStyle: .dark))
            .cgColor.converted(to: colorSpace, intent: .defaultIntent, options: nil)
        let end = second.resolvedColor(with: UITraitCollection(userInterfaceStyle: .dark))
            .cgColor.converted(to: colorSpace, intent: .defaultIntent, options: nil)
        UIColor(cgColor: start ?? first.cgColor).getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
        UIColor(cgColor: end ?? second.cgColor).getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
        let amount = min(1, max(0, amount))
        return UIColor(
            red: r1 + (r2 - r1) * amount,
            green: g1 + (g2 - g1) * amount,
            blue: b1 + (b2 - b1) * amount,
            alpha: a1 + (a2 - a1) * amount
        )
    }
}

@MainActor
private final class DisplayLinkProxy: NSObject {
    weak var target: DotOrbView?

    @objc func step(_ link: CADisplayLink) {
        target?.displayTick()
    }
}
