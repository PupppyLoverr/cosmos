//! Styles, read from the current skin (`skin::palette`). The upper-case
//! names are the tokens the renderer has always used; they are functions
//! now so a theme change — or a live `/customise` edit — repaints everything
//! from the next frame on.
#![allow(non_snake_case)]

use ratatui_core::style::{Color, Modifier, Style};

use crate::skin;

fn p() -> skin::Palette {
    skin::palette()
}

/// Secondary text: hints, meta, descriptions.
pub fn DIM() -> Style {
    Style::new().fg(p().dim)
}
/// Horizontal rules around the composer.
pub fn RULE() -> Style {
    Style::new().fg(p().rule)
}
/// Faint placeholder / ghost completion.
pub fn GHOST() -> Style {
    Style::new().fg(p().ghost)
}

pub fn ACCENT() -> Color {
    p().accent
}
pub fn GREEN() -> Color {
    p().success
}
pub fn YELLOW() -> Color {
    p().warn
}
pub fn RED() -> Color {
    p().error
}
pub fn ORANGE() -> Color {
    p().shell
}
pub fn USER_BG() -> Color {
    p().user_bg
}
pub fn DIFF_ADD_BG() -> Color {
    p().diff_add_bg
}
pub fn DIFF_DEL_BG() -> Color {
    p().diff_del_bg
}

pub fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

/// `fg` that leaves the colour unset for the terminal default, so a theme
/// that keeps the default renders exactly like an unstyled span.
pub fn fg_or_default(c: Color) -> Style {
    if c == Color::Reset {
        Style::new()
    } else {
        Style::new().fg(c)
    }
}

pub fn bold(s: Style) -> Style {
    s.add_modifier(Modifier::BOLD)
}

pub fn ACCENT_BOLD() -> Style {
    Style::new().fg(ACCENT()).add_modifier(Modifier::BOLD)
}
pub fn CODE_INLINE() -> Style {
    Style::new().fg(p().code)
}
