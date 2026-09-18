// Adaptive monochrome theme — a direct port of crates/ui/src/theme.rs.
//
// Colors are computed from the same oklch definitions the desktop app uses
// (Björn Ottosson's OKLab matrices, the ones CSS Color 4 specifies), so every
// surface and accent lands on identical sRGB values in each appearance.
// **Numbers drive layout, colors are paint**: layout constants are plain
// numbers and never depend on which color is painted.
//
// Every semantic color is a UIColor dynamic provider: dark keeps the
// authored desktop palette, light is its deliberate inverse — same oklch
// tones mirrored around mid-grey, alpha washes rebased from white to black.

import SwiftUI

enum Theme {
    // ---- paint: neutral surfaces (oklch chroma 0) ----
    /// Main panel background.
    static let bg = dynamic(dark: grey(6), light: neutral(0.985))
    /// Shell / sidebar surface.
    static let surface = dynamic(dark: grey(13), light: .white)
    /// Raised surface: popovers, dialogs, cards.
    static let surfaceRaised = dynamic(dark: neutral(0.235), light: neutral(0.925))
    /// Hover/pressed wash for interactive rows.
    static let elementHover = dynamic(dark: whiteAlpha(0.06), light: blackAlpha(0.05))
    /// Active/selected wash.
    static let elementActive = dynamic(dark: whiteAlpha(0.10), light: blackAlpha(0.08))
    /// Hairline border — reads on any surface.
    static let border = dynamic(dark: whiteAlpha(0.08), light: blackAlpha(0.10))
    /// Stronger border for focused/raised edges.
    static let borderStrong = dynamic(dark: whiteAlpha(0.14), light: blackAlpha(0.16))

    // ---- paint: text ----
    static let text = dynamic(dark: neutral(0.922), light: neutral(0.14))      // ~neutral-200 / ~neutral-900
    static let textMuted = dynamic(dark: neutral(0.708), light: neutral(0.42)) // ~neutral-400 / ~neutral-600
    static let textFaint = dynamic(dark: neutral(0.556), light: neutral(0.52)) // ~neutral-500

    // ---- paint: accents ----
    // Dark keeps indigo-400; light steps down to indigo-500 for contrast.
    static let accent = dynamic(dark: oklch(0.673, 0.182, 276.935),
                                light: oklch(0.545, 0.215, 277.5))
    static let accentStrong = dynamic(dark: oklch(0.585, 0.233, 277.117),
                                      light: oklch(0.48, 0.21, 277.5))
    static let danger = dynamic(dark: oklch(0.704, 0.191, 22.216),
                                light: oklch(0.577, 0.215, 27.325))
    static let dangerSoft = dynamic(dark: oklch(0.808, 0.114, 19.571),
                                    light: oklch(0.637, 0.183, 23.0))
    static let warning = dynamic(dark: oklch(0.828, 0.189, 84.429),
                                 light: oklch(0.62, 0.16, 75.0))

    // ---- paint: status dots (shell/spaces.rs status_dot_color) ----
    static let statusWorking = dynamic(dark: oklch(0.718, 0.202, 349.761),   // pink-400
                                       light: oklch(0.60, 0.19, 350.0))
    static let statusCompleted = dynamic(dark: oklch(0.765, 0.177, 163.223), // emerald-400
                                         light: oklch(0.56, 0.15, 163.0))
    /// Claude brand orange — kept even on the mono surface.
    static let claudeBrand = Color(red: 0xD9 / 255.0, green: 0x77 / 255.0, blue: 0x57 / 255.0)

    // ---- paint: markdown inline code (violet family) ----
    static let inlineCodeText = dynamic(dark: oklch(0.811, 0.111, 293.571),
                                        light: oklch(0.45, 0.14, 293.5))
    static let inlineCodeWash = dynamic(
        dark: oklch(0.702, 0.183, 293.541).opacity(0.12),
        light: oklch(0.55, 0.16, 293.5).opacity(0.10))

    // ---- paint: syntax tokens (soft, paint-only) ----
    static let tokenKeyword = dynamic(dark: oklch(0.709, 0.129, 20.0),
                                      light: oklch(0.52, 0.13, 20.0))   // soft rose
    static let tokenString = dynamic(dark: oklch(0.770, 0.110, 168.0),
                                     light: oklch(0.52, 0.11, 165.0))   // soft green
    static let tokenNumber = dynamic(dark: oklch(0.780, 0.120, 80.0),
                                     light: oklch(0.55, 0.11, 78.0))    // soft amber

    // ---- numbers drive layout (pt) ----
    static let bubbleRadius: CGFloat = 22
    static let panelRadius: CGFloat = 10
    static let controlRadius: CGFloat = 6
    static let spaceXS: CGFloat = 4
    static let spaceSM: CGFloat = 8
    static let spaceMD: CGFloat = 12
    static let spaceLG: CGFloat = 16
}

// MARK: - Fonts

extension Theme {
    static let fontSansName = "Geist"
    static let fontMonoName = "GeistMono-Regular"

    static func sans(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        // Static weight cuts register as separate families — select by
        // PostScript name so weights actually resolve.
        let name: String
        if weight == .medium {
            name = "Geist-Medium"
        } else if weight == .semibold {
            name = "Geist-SemiBold"
        } else if weight == .bold {
            name = "Geist-Bold"
        } else {
            name = "Geist-Regular"
        }
        return .custom(name, size: size)
    }

    static func mono(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .custom(fontMonoName, size: size).weight(weight)
    }

    static func sansUI(_ size: CGFloat, weight: UIFont.Weight = .regular) -> UIFont {
        let traits: [UIFontDescriptor.TraitKey: Any] = [.weight: weight]
        let descriptor = UIFontDescriptor(fontAttributes: [
            .family: "Geist",
            .traits: traits,
        ])
        return UIFont(descriptor: descriptor, size: size)
    }

    static func monoUI(_ size: CGFloat) -> UIFont {
        UIFont(name: fontMonoName, size: size)
            ?? .monospacedSystemFont(ofSize: size, weight: .regular)
    }
}

// MARK: - Color primitives (ported from theme.rs)

/// One color per appearance — resolved by the trait environment, so a
/// `Theme.` static adapts everywhere without touching call sites.
func dynamic(dark: Color, light: Color) -> Color {
    Color(uiColor: UIColor(dynamicProvider: { traits in
        traits.userInterfaceStyle == .dark ? UIColor(dark) : UIColor(light)
    }))
}

/// A neutral (chroma 0) oklch tone. Chroma 0 means r == g == b exactly.
func neutral(_ lightness: Double) -> Color {
    let v = Double(oklchToSrgb(l: lightness, c: 0, hDeg: 0)[0])
    return Color(red: v, green: v, blue: v)
}

/// White at the given alpha — the dark-mode hairline/wash primitive.
func whiteAlpha(_ alpha: Double) -> Color {
    Color.white.opacity(alpha)
}

/// Black at the given alpha — the light-mode hairline/wash primitive.
func blackAlpha(_ alpha: Double) -> Color {
    Color.black.opacity(alpha)
}

/// An exact achromatic tone from an 8-bit channel value (`grey(13)` ≡ #0d0d0d).
func grey(_ value: UInt8) -> Color {
    let v = Double(value) / 255.0
    return Color(red: v, green: v, blue: v)
}

/// oklch (CSS notation: L 0..1, C, H degrees) → sRGB Color.
func oklch(_ l: Double, _ c: Double, _ hDeg: Double) -> Color {
    let rgb = oklchToSrgb(l: l, c: c, hDeg: hDeg)
    return Color(red: Double(rgb[0]), green: Double(rgb[1]), blue: Double(rgb[2]))
}

/// oklch → sRGB (each 0..1, clamped/gamut-clipped per channel).
func oklchToSrgb(l: Double, c: Double, hDeg: Double) -> [Double] {
    let h = hDeg * .pi / 180
    let a = c * cos(h)
    let b = c * sin(h)

    // OKLab → LMS (cube roots undone)
    let l_ = l + 0.39633778 * a + 0.21580376 * b
    let m_ = l - 0.105561346 * a - 0.06385417 * b
    let s_ = l - 0.08948418 * a - 1.2914855 * b
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_)

    // LMS → linear sRGB
    let r = 4.0767417 * l3 - 3.3077116 * m3 + 0.23096993 * s3
    let g = -1.268438 * l3 + 2.6097574 * m3 - 0.3413194 * s3
    let bl = -0.0041960863 * l3 - 0.7034186 * m3 + 1.7076147 * s3

    return [gammaEncode(r), gammaEncode(g), gammaEncode(bl)]
}

private func gammaEncode(_ x: Double) -> Double {
    let x = min(max(x, 0), 1)
    return x <= 0.0031308 ? 12.92 * x : 1.055 * pow(x, 1.0 / 2.4) - 0.055
}
