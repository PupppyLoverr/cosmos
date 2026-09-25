//! Palette — sampled from the Devin CLI reference: near-white body text on
//! the terminal's own background, a soft grey for everything secondary, a
//! darker grey for rules, one cyan accent for selection, and a filled grey
//! bar behind user turns.

use ratatui::style::{Color, Modifier, Style};

/// Secondary text: hints, meta, descriptions.
pub const DIM: Style = Style::new().fg(Color::Rgb(0x8a, 0x8a, 0x8a));
/// Horizontal rules around the composer.
pub const RULE: Style = Style::new().fg(Color::Rgb(0x4a, 0x4a, 0x4a));
/// Faint placeholder / ghost completion.
pub const GHOST: Style = Style::new().fg(Color::Rgb(0x6c, 0x6c, 0x6c));

pub const ACCENT: Color = Color::Rgb(0x5c, 0xd0, 0xff);
pub const GREEN: Color = Color::Rgb(0x5f, 0xd7, 0x87);
pub const YELLOW: Color = Color::Rgb(0xe5, 0xd8, 0x92);
pub const RED: Color = Color::Rgb(0xff, 0x6b, 0x6b);
pub const ORANGE: Color = Color::Rgb(0xf0, 0xa0, 0x5a);
pub const USER_BG: Color = Color::Rgb(0x26, 0x26, 0x26);
pub const DIFF_ADD_BG: Color = Color::Rgb(0x12, 0x2e, 0x1c);
pub const DIFF_DEL_BG: Color = Color::Rgb(0x3a, 0x16, 0x18);

pub fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

pub fn bold(s: Style) -> Style {
    s.add_modifier(Modifier::BOLD)
}

pub const ACCENT_BOLD: Style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
pub const CODE_INLINE: Style = Style::new().fg(Color::Rgb(0xe6, 0xb4, 0x73));
