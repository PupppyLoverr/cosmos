//! The zcli mark: a small black-and-white Milky Way drawn in braille.
//!
//! A tilted two-arm logarithmic spiral with a bright core and a sprinkle of
//! field stars is sampled on a 2×4-per-cell dot grid, then each cell is
//! shaded by how much light it holds (core white → arms grey → dust dim).
//! Deterministic — the same galaxy every launch.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub const COLS: usize = 12;
pub const ROWS: usize = 4;
const W: usize = COLS * 2;
const H: usize = ROWS * 4;

/// Cheap integer hash → [0, 1).
fn noise(x: usize, y: usize, salt: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ salt;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0xFFFF) as f32 / 65536.0
}

/// Light at one dot of the grid (0 = empty sky).
fn light(x: usize, y: usize) -> f32 {
    let cx = (W as f32 - 1.0) / 2.0;
    let cy = (H as f32 - 1.0) / 2.0;
    // normalised so the disk spans the grid
    let dx = (x as f32 - cx) / (W as f32 / 2.0);
    let dy = (y as f32 - cy) / (H as f32 / 2.0);
    // tilt the disk, then undo the viewing inclination
    let a: f32 = -0.4;
    let u = dx * a.cos() + dy * a.sin();
    let v = (-dx * a.sin() + dy * a.cos()) / 0.85;
    let r = (u * u + v * v).sqrt();
    let theta = v.atan2(u);

    let core = (-(r / 0.25).powi(2)).exp() * 1.6;
    // two arms wound as a logarithmic spiral
    let wind = theta - 3.0 * (r + 0.05).ln();
    let arm = ((1.0 + (2.0 * wind).cos()) / 2.0).powf(1.5);
    let disk = if r < 1.05 { (-(r / 0.9)).exp() } else { 0.0 };
    let dust = arm * disk * 2.2 + disk * 0.18;
    // a few field stars beyond the disk
    let stars = if noise(x, y, 7) > 0.975 && r > 0.8 { 0.8 } else { 0.0 };
    core + dust + stars
}

struct Cell {
    ch: char,
    lum: f32,
}

fn grid() -> &'static Vec<Vec<Cell>> {
    static G: OnceLock<Vec<Vec<Cell>>> = OnceLock::new();
    G.get_or_init(|| {
        // braille bit for (column-in-cell, row-in-cell)
        const BIT: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        let mut rows = Vec::with_capacity(ROWS);
        for cy in 0..ROWS {
            let mut row = Vec::with_capacity(COLS);
            for cx in 0..COLS {
                let (mut bits, mut sum, mut on) = (0u32, 0f32, 0u32);
                for (ix, col) in BIT.iter().enumerate() {
                    for (iy, bit) in col.iter().enumerate() {
                        let (x, y) = (cx * 2 + ix, cy * 4 + iy);
                        let l = light(x, y);
                        // dither: jitter the threshold so edges break up into dust
                        if l + (noise(x, y, 31) - 0.5) * 0.35 > 0.34 {
                            bits |= bit;
                            sum += l;
                            on += 1;
                        }
                    }
                }
                let ch = char::from_u32(0x2800 + bits).unwrap_or(' ');
                let lum = if on == 0 { 0.0 } else { sum / on as f32 };
                row.push(Cell { ch: if bits == 0 { ' ' } else { ch }, lum });
            }
            rows.push(row);
        }
        rows
    })
}

pub(crate) fn shade(lum: f32) -> Style {
    if lum > 1.0 {
        Style::new().fg(Color::Rgb(0xf4, 0xf4, 0xf4)).add_modifier(Modifier::BOLD)
    } else if lum > 0.55 {
        Style::new().fg(Color::Rgb(0xc4, 0xc4, 0xc4))
    } else {
        Style::new().fg(Color::Rgb(0x7c, 0x7c, 0x7c))
    }
}

/// One row of the mark as styled spans (exactly `COLS` columns wide).
pub fn row(r: usize) -> Vec<Span<'static>> {
    grid()[r]
        .iter()
        .map(|c| Span::styled(c.ch.to_string(), shade(c.lum)))
        .collect()
}

/// Startup banner: the galaxy with the name + version beside it, Devin-style.
pub fn banner(name: &str, version: &str) -> Vec<Line<'static>> {
    let mut out = vec![Line::default()];
    for r in 0..ROWS {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(row(r));
        spans.push(Span::raw("   "));
        match r {
            1 => spans.push(Span::raw(name.to_string())),
            2 => spans.push(Span::styled(version.to_string(), crate::theme::DIM)),
            _ => {}
        }
        out.push(Line::from(spans));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn galaxy_has_shape() {
        let lit: usize = grid().iter().flatten().filter(|c| c.ch != ' ').count();
        assert!(lit > COLS * ROWS / 3, "galaxy too sparse: {lit}");
        assert!(grid().iter().all(|r| r.len() == COLS));
    }

    #[test]
    fn print_galaxy() {
        // run with --nocapture to eyeball it
        for l in banner("zcli", "v0.1.0") {
            let s: String = l.spans.iter().map(|s| s.content.to_string()).collect();
            println!("{s}");
        }
    }
}
