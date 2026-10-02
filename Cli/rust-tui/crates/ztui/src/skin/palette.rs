//! Colour tokens and the built-in themes. Every colour the TUI paints comes
//! from a `Palette` field; a theme is a palette, and `ui.toml` overrides
//! individual tokens by name.
// TOKENS, show_color and Palette::get also serve /customise (ui_get).
#![allow(dead_code)]

use ratatui_core::style::Color;

/// Every colour token, by name (the keys `[theme.colors]` accepts).
pub const TOKENS: &[&str] = &[
    "text", "accent", "success", "warn", "error", "shell", "dim", "rule", "ghost", "user_bg",
    "diff_add_bg", "diff_del_bg", "diff_add_word", "diff_del_word", "code", "code_bg", "heading",
    "heading_sub", "link", "quote", "badge_fg", "logo_bright", "logo_mid", "logo_dim",
    "syntax.base", "syntax.comment", "syntax.keyword", "syntax.string", "syntax.escape",
    "syntax.number", "syntax.function", "syntax.type", "syntax.tag", "syntax.attribute",
    "syntax.operator", "syntax.variable", "syntax.punctuation",
];

/// Built-in theme names.
pub const THEMES: &[&str] = &["cosmos", "classic", "nebula", "mono", "solar", "light", "high-contrast"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// body text; `Color::Reset` = the terminal's own foreground
    pub text: Color,
    pub accent: Color,
    pub success: Color,
    pub warn: Color,
    pub error: Color,
    /// `!` shell mode
    pub shell: Color,
    pub dim: Color,
    pub rule: Color,
    pub ghost: Color,
    pub user_bg: Color,
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
    pub diff_add_word: Color,
    pub diff_del_word: Color,
    pub code: Color,
    pub code_bg: Color,
    pub heading: Color,
    pub heading_sub: Color,
    pub link: Color,
    pub quote: Color,
    pub badge_fg: Color,
    pub logo_bright: Color,
    pub logo_mid: Color,
    pub logo_dim: Color,
    pub syn_base: Color,
    pub syn_comment: Color,
    pub syn_keyword: Color,
    pub syn_string: Color,
    pub syn_escape: Color,
    pub syn_number: Color,
    pub syn_function: Color,
    pub syn_type: Color,
    pub syn_tag: Color,
    pub syn_attribute: Color,
    pub syn_operator: Color,
    pub syn_variable: Color,
    pub syn_punct: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The look cosmostui shipped with before themes existed.
pub const CLASSIC: Palette = Palette {
    text: Color::Reset,
    accent: rgb(0x5cd0ff),
    success: rgb(0x5fd787),
    warn: rgb(0xe5d892),
    error: rgb(0xff6b6b),
    shell: rgb(0xf0a05a),
    dim: rgb(0x8a8a8a),
    rule: rgb(0x4a4a4a),
    ghost: rgb(0x6c6c6c),
    user_bg: rgb(0x262626),
    diff_add_bg: rgb(0x122e1c),
    diff_del_bg: rgb(0x3a1618),
    diff_add_word: rgb(0x1f5a35),
    diff_del_word: rgb(0x6a2228),
    code: rgb(0xe6b473),
    code_bg: rgb(0x1c1c1c),
    heading: Color::Reset,
    heading_sub: rgb(0xc8c8c8),
    link: rgb(0x5cd0ff),
    quote: rgb(0x8a8a8a),
    badge_fg: Color::Black,
    logo_bright: rgb(0xf4f4f4),
    logo_mid: rgb(0xc4c4c4),
    logo_dim: rgb(0x7c7c7c),
    syn_base: rgb(0xd4d4d4),
    syn_comment: rgb(0x6a737d),
    syn_keyword: rgb(0xc68ff2),
    syn_string: rgb(0x9ece6a),
    syn_escape: rgb(0x89ddff),
    syn_number: rgb(0xff9e64),
    syn_function: rgb(0x7aa2f7),
    syn_type: rgb(0x2ac3de),
    syn_tag: rgb(0xf7768e),
    syn_attribute: rgb(0x73daca),
    syn_operator: rgb(0x89ddff),
    syn_variable: rgb(0xe0af68),
    syn_punct: rgb(0x9aa5b8),
};

/// Deep-space default: periwinkle accent, soft mint/rose status colours.
pub const COSMOS: Palette = Palette {
    accent: rgb(0x9aa8ff),
    success: rgb(0x7ee2b8),
    warn: rgb(0xf3d47c),
    error: rgb(0xff7a90),
    shell: rgb(0xf7a86b),
    dim: rgb(0x8c8fa3),
    rule: rgb(0x3a3e55),
    ghost: rgb(0x5f637a),
    user_bg: rgb(0x1f2133),
    diff_add_bg: rgb(0x112b22),
    diff_del_bg: rgb(0x341622),
    diff_add_word: rgb(0x1d5a43),
    diff_del_word: rgb(0x6b2236),
    code: rgb(0xf2b880),
    code_bg: rgb(0x181a28),
    heading: rgb(0xc7ceff),
    heading_sub: rgb(0xb5b9d4),
    link: rgb(0x8fd3ff),
    quote: rgb(0x9a9db3),
    logo_bright: rgb(0xe9ebff),
    logo_mid: rgb(0xa9b0ee),
    logo_dim: rgb(0x5d628f),
    ..CLASSIC
};

pub const NEBULA: Palette = Palette {
    accent: rgb(0xd68cff),
    success: rgb(0x8cf0c8),
    warn: rgb(0xffd479),
    error: rgb(0xff6fa3),
    dim: rgb(0x9a8aad),
    rule: rgb(0x47375c),
    ghost: rgb(0x6d5c80),
    user_bg: rgb(0x2a1d38),
    code: rgb(0xffb3e6),
    code_bg: rgb(0x1f1529),
    heading: rgb(0xf0c4ff),
    heading_sub: rgb(0xd2b4e6),
    link: rgb(0xa8c7ff),
    quote: rgb(0xb19bc4),
    logo_bright: rgb(0xffe3ff),
    logo_mid: rgb(0xd68cff),
    logo_dim: rgb(0x6e3f8f),
    ..COSMOS
};

pub const MONO: Palette = Palette {
    accent: rgb(0xf0f0f0),
    success: rgb(0xd0d0d0),
    warn: rgb(0xbdbdbd),
    error: rgb(0xffffff),
    shell: rgb(0xd0d0d0),
    dim: rgb(0x8a8a8a),
    rule: rgb(0x444444),
    ghost: rgb(0x666666),
    user_bg: rgb(0x262626),
    diff_add_bg: rgb(0x2b2b2b),
    diff_del_bg: rgb(0x1a1a1a),
    diff_add_word: rgb(0x4a4a4a),
    diff_del_word: rgb(0x0e0e0e),
    code: rgb(0xdcdcdc),
    heading: rgb(0xffffff),
    heading_sub: rgb(0xc8c8c8),
    link: rgb(0xe0e0e0),
    syn_comment: rgb(0x707070),
    syn_keyword: rgb(0xffffff),
    syn_string: rgb(0xbdbdbd),
    syn_escape: rgb(0xbdbdbd),
    syn_number: rgb(0xe0e0e0),
    syn_function: rgb(0xe8e8e8),
    syn_type: rgb(0xd0d0d0),
    syn_tag: rgb(0xd0d0d0),
    syn_attribute: rgb(0xc0c0c0),
    syn_operator: rgb(0xa0a0a0),
    syn_variable: rgb(0xd8d8d8),
    syn_punct: rgb(0x909090),
    ..CLASSIC
};

pub const SOLAR: Palette = Palette {
    accent: rgb(0xffb454),
    success: rgb(0xa8d873),
    warn: rgb(0xffd866),
    error: rgb(0xff6e5a),
    shell: rgb(0xff9e3d),
    dim: rgb(0xa39887),
    rule: rgb(0x52463a),
    ghost: rgb(0x75685a),
    user_bg: rgb(0x2b241d),
    code: rgb(0xffcf8a),
    code_bg: rgb(0x221c16),
    heading: rgb(0xffe0b0),
    heading_sub: rgb(0xe6c9a0),
    link: rgb(0xffc87a),
    quote: rgb(0xb0a08a),
    logo_bright: rgb(0xfff2d8),
    logo_mid: rgb(0xffb454),
    logo_dim: rgb(0x8a5a22),
    ..CLASSIC
};

/// For light terminal backgrounds.
pub const LIGHT: Palette = Palette {
    text: Color::Reset,
    accent: rgb(0x2457d6),
    success: rgb(0x1a7f45),
    warn: rgb(0x9a6b00),
    error: rgb(0xc4253c),
    shell: rgb(0xb4580f),
    dim: rgb(0x6b6f7a),
    rule: rgb(0xc9ccd4),
    ghost: rgb(0x9a9ea8),
    user_bg: rgb(0xeceef3),
    diff_add_bg: rgb(0xdff3e6),
    diff_del_bg: rgb(0xf8e0e3),
    diff_add_word: rgb(0xb6e3c5),
    diff_del_word: rgb(0xf0bcc4),
    code: rgb(0x9a3d0b),
    code_bg: rgb(0xf3f4f7),
    heading: rgb(0x1c2a5c),
    heading_sub: rgb(0x3b4a7a),
    link: rgb(0x2457d6),
    quote: rgb(0x5d6270),
    badge_fg: Color::White,
    logo_bright: rgb(0x1c2a5c),
    logo_mid: rgb(0x4b5fa8),
    logo_dim: rgb(0x9aa3c7),
    syn_base: rgb(0x2b2f3a),
    syn_comment: rgb(0x8a8f99),
    syn_keyword: rgb(0x8a2fc9),
    syn_string: rgb(0x2f7d16),
    syn_escape: rgb(0x0a6f99),
    syn_number: rgb(0xb4580f),
    syn_function: rgb(0x2457d6),
    syn_type: rgb(0x00808a),
    syn_tag: rgb(0xc4253c),
    syn_attribute: rgb(0x0a7f73),
    syn_operator: rgb(0x0a6f99),
    syn_variable: rgb(0x8a5a00),
    syn_punct: rgb(0x5d6270),
};

pub const HIGH_CONTRAST: Palette = Palette {
    accent: rgb(0x00e5ff),
    success: rgb(0x00ff7f),
    warn: rgb(0xffff00),
    error: rgb(0xff3b3b),
    shell: rgb(0xff9900),
    dim: rgb(0xd0d0d0),
    rule: rgb(0x9a9a9a),
    ghost: rgb(0xb0b0b0),
    user_bg: rgb(0x303030),
    diff_add_bg: rgb(0x004d26),
    diff_del_bg: rgb(0x5c0010),
    code: rgb(0xffd166),
    heading: rgb(0xffffff),
    heading_sub: rgb(0xf0f0f0),
    link: rgb(0x66ccff),
    quote: rgb(0xe0e0e0),
    ..CLASSIC
};

pub fn builtin(name: &str) -> Option<Palette> {
    Some(match name {
        "cosmos" => COSMOS,
        "classic" => CLASSIC,
        "nebula" => NEBULA,
        "mono" => MONO,
        "solar" => SOLAR,
        "light" => LIGHT,
        "high-contrast" | "contrast" => HIGH_CONTRAST,
        _ => return None,
    })
}

/// `#rrggbb`, `#rgb`, an ANSI name (`red`, `bright_blue`, …) or `default`.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let full = match h.len() {
            6 => h.to_string(),
            3 => h.chars().flat_map(|c| [c, c]).collect(),
            _ => return None,
        };
        let v = u32::from_str_radix(&full, 16).ok()?;
        return Some(rgb(v));
    }
    Some(match s.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
        "default" | "reset" | "inherit" | "none" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "dark_gray" | "dark_grey" | "bright_black" => Color::DarkGray,
        "bright_red" | "light_red" => Color::LightRed,
        "bright_green" | "light_green" => Color::LightGreen,
        "bright_yellow" | "light_yellow" => Color::LightYellow,
        "bright_blue" | "light_blue" => Color::LightBlue,
        "bright_magenta" | "light_magenta" => Color::LightMagenta,
        "bright_cyan" | "light_cyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    })
}

/// `#rrggbb` for RGB colours, the ANSI name otherwise (for `ui_get`).
pub fn show_color(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => "default".into(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

impl Palette {
    fn slot(&mut self, key: &str) -> Option<&mut Color> {
        Some(match key {
            "text" => &mut self.text,
            "accent" => &mut self.accent,
            "success" => &mut self.success,
            "warn" => &mut self.warn,
            "error" => &mut self.error,
            "shell" => &mut self.shell,
            "dim" => &mut self.dim,
            "rule" => &mut self.rule,
            "ghost" => &mut self.ghost,
            "user_bg" => &mut self.user_bg,
            "diff_add_bg" => &mut self.diff_add_bg,
            "diff_del_bg" => &mut self.diff_del_bg,
            "diff_add_word" => &mut self.diff_add_word,
            "diff_del_word" => &mut self.diff_del_word,
            "code" => &mut self.code,
            "code_bg" => &mut self.code_bg,
            "heading" => &mut self.heading,
            "heading_sub" => &mut self.heading_sub,
            "link" => &mut self.link,
            "quote" => &mut self.quote,
            "badge_fg" => &mut self.badge_fg,
            "logo_bright" => &mut self.logo_bright,
            "logo_mid" => &mut self.logo_mid,
            "logo_dim" => &mut self.logo_dim,
            "syntax.base" => &mut self.syn_base,
            "syntax.comment" => &mut self.syn_comment,
            "syntax.keyword" => &mut self.syn_keyword,
            "syntax.string" => &mut self.syn_string,
            "syntax.escape" => &mut self.syn_escape,
            "syntax.number" => &mut self.syn_number,
            "syntax.function" => &mut self.syn_function,
            "syntax.type" => &mut self.syn_type,
            "syntax.tag" => &mut self.syn_tag,
            "syntax.attribute" => &mut self.syn_attribute,
            "syntax.operator" => &mut self.syn_operator,
            "syntax.variable" => &mut self.syn_variable,
            "syntax.punctuation" => &mut self.syn_punct,
            _ => return None,
        })
    }

    /// Override one token; false when the key is unknown.
    pub fn set(&mut self, key: &str, c: Color) -> bool {
        match self.slot(key) {
            Some(s) => {
                *s = c;
                true
            }
            None => false,
        }
    }

    pub fn get(&self, key: &str) -> Option<Color> {
        let mut copy = *self;
        copy.slot(key).map(|c| *c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_has_a_slot_and_every_theme_resolves() {
        let mut p = CLASSIC;
        for t in TOKENS {
            assert!(p.set(t, Color::Red), "{t}");
            assert_eq!(p.get(t), Some(Color::Red));
        }
        assert!(!p.set("permissions", Color::Red));
        for t in THEMES {
            assert!(builtin(t).is_some(), "{t}");
        }
    }

    #[test]
    fn colours_parse() {
        assert_eq!(parse_color("#5cd0ff"), Some(Color::Rgb(0x5c, 0xd0, 0xff)));
        assert_eq!(parse_color("#fff"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(parse_color("bright-blue"), Some(Color::LightBlue));
        assert_eq!(parse_color("default"), Some(Color::Reset));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(show_color(Color::Rgb(1, 2, 3)), "#010203");
    }
}
