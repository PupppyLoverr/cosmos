//! Real images of the pet, for terminals that can show them: sixel (Windows
//! Terminal 1.22+, WezTerm, iTerm2, foot, mlterm…) or the kitty graphics
//! protocol (kitty, Ghostty). Everywhere else the pet is half-block pixel
//! art, which needs nothing from the terminal.
//!
//! Images only go into committed scrollback (the `/buddy` card, `/pettalk`
//! replies), never the redrawn live region. A block reserves `rows` blank
//! rows; its last row carries a zero-width marker (the same tag-character
//! trick as OSC 8 links), and `term::write_line` draws the image back over
//! the reserved rows when it reaches it — the rows already exist by then,
//! so drawing never scrolls the screen.

use std::collections::HashMap;
use std::sync::Mutex;

use ratatui_core::text::{Line, Span};

use crate::buddy::{palette_for, Identity};
use crate::pixel::{self, Pose, Rgb};
use crate::skin::BuddyCfg;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Protocol {
    Sixel,
    Kitty,
}

/// What the terminal can show, from `[buddy] images` or a guess from the
/// environment.
pub fn protocol(cfg: &BuddyCfg) -> Option<Protocol> {
    match cfg.images.as_str() {
        "sixel" => Some(Protocol::Sixel),
        "kitty" => Some(Protocol::Kitty),
        "off" => None,
        _ => detect(),
    }
}

fn detect() -> Option<Protocol> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let term = env("TERM");
    let prog = env("TERM_PROGRAM");
    if std::env::var_os("KITTY_WINDOW_ID").is_some() || term.contains("kitty") || prog == "ghostty" || term.contains("ghostty") {
        return Some(Protocol::Kitty);
    }
    if std::env::var_os("WT_SESSION").is_some()
        || matches!(prog.as_str(), "WezTerm" | "iTerm.app" | "mintty" | "contour")
        || term.contains("foot")
        || term.contains("mlterm")
    {
        return Some(Protocol::Sixel);
    }
    None
}

/// A placed image: how many rows and columns it reserves, and its id.
pub struct Block {
    pub rows: usize,
    pub cols: usize,
    id: u64,
}

struct Stored {
    bytes: Vec<u8>,
    rows: usize,
}

static IMAGES: Mutex<Option<HashMap<u64, Stored>>> = Mutex::new(None);
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Block {
    /// Put the marker on `line` (the block's last reserved row).
    pub fn mark(&self, line: Line<'static>) -> Line<'static> {
        let mut spans = vec![Span::raw(crate::markdown::link_mark(&format!("img:{}", self.id)))];
        spans.extend(line.spans);
        Line::from(spans)
    }
}

/// Assumed cell size in pixels (the common default; images are sized to
/// fit inside the reserved box even on somewhat smaller fonts).
const CELL_W: usize = 8;
const CELL_H: usize = 16;

/// Encode the pet as an image and reserve room for it. None when the
/// terminal can't show images (the caller draws half-blocks instead).
pub fn block(id: &Identity, cfg: &BuddyCfg, pose: Pose, scale: usize, min_rows: usize) -> Option<Block> {
    let proto = protocol(cfg)?;
    let px = pixel::canvas(id.species(), &palette_for(id, cfg), pose);
    let (w, h) = (pixel::W * scale, pixel::CANVAS_H * scale);
    let cols = w.div_ceil(CELL_W) + 1;
    let rows = (h.div_ceil(CELL_H) + 1).max(min_rows);
    let bytes = match proto {
        Protocol::Sixel => sixel(&px, scale),
        Protocol::Kitty => kitty(&px, scale, cols - 1, h.div_ceil(CELL_H)),
    };
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut g = IMAGES.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new).insert(n, Stored { bytes, rows });
    Some(Block { rows, cols, id: n })
}

/// The bytes that draw image `url` (`img:<id>`) over the `rows - 1` rows above
/// the cursor and come back to where the cursor was.
pub fn draw(url: &str) -> Option<Vec<u8>> {
    let id: u64 = url.strip_prefix("img:")?.parse().ok()?;
    let g = IMAGES.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.as_ref()?.get(&id)?;
    let mut out = Vec::with_capacity(s.bytes.len() + 16);
    out.extend_from_slice(b"\x1b7"); // save cursor
    if s.rows > 1 {
        out.extend_from_slice(format!("\x1b[{}A", s.rows - 1).as_bytes());
    }
    out.extend_from_slice(b"\r\x1b[1C");
    out.extend_from_slice(&s.bytes);
    out.extend_from_slice(b"\x1b8"); // and back
    Some(out)
}

type Canvas = [[Option<Rgb>; pixel::W]; pixel::CANVAS_H];

/// Sixel with transparent background, `scale`× nearest-neighbour.
pub fn sixel(px: &Canvas, scale: usize) -> Vec<u8> {
    let (w, h) = (pixel::W * scale, pixel::CANVAS_H * scale);
    let at = |x: usize, y: usize| px[y / scale][x / scale];
    // the palette: every distinct colour
    let mut colours: Vec<Rgb> = Vec::new();
    for row in px.iter() {
        for c in row.iter().flatten() {
            if !colours.contains(c) {
                colours.push(*c);
            }
        }
    }
    let pct = |v: u8| (v as u32 * 100 + 127) / 255;
    let mut out = format!("\x1bP0;1;0q\"1;1;{w};{h}").into_bytes();
    for (i, c) in colours.iter().enumerate() {
        out.extend_from_slice(format!("#{i};2;{};{};{}", pct(c.0), pct(c.1), pct(c.2)).as_bytes());
    }
    for band in 0..h.div_ceil(6) {
        let mut first = true;
        for (ci, c) in colours.iter().enumerate() {
            let cols: Vec<u8> = (0..w)
                .map(|x| {
                    let mut bits = 0u8;
                    for k in 0..6 {
                        let y = band * 6 + k;
                        if y < h && at(x, y) == Some(*c) {
                            bits |= 1 << k;
                        }
                    }
                    bits
                })
                .collect();
            if cols.iter().all(|b| *b == 0) {
                continue;
            }
            if !first {
                out.push(b'$');
            }
            first = false;
            out.extend_from_slice(format!("#{ci}").as_bytes());
            // run-length encode
            let mut i = 0;
            while i < cols.len() {
                let mut j = i;
                while j < cols.len() && cols[j] == cols[i] {
                    j += 1;
                }
                let ch = 63 + cols[i];
                let n = j - i;
                if n > 3 {
                    out.extend_from_slice(format!("!{n}").as_bytes());
                    out.push(ch);
                } else {
                    out.extend(std::iter::repeat_n(ch, n));
                }
                i = j;
            }
        }
        out.push(b'-');
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Kitty graphics protocol: raw RGBA, shown in a `cols`×`rows` cell box,
/// cursor left where it was.
pub fn kitty(px: &Canvas, scale: usize, cols: usize, rows: usize) -> Vec<u8> {
    let (w, h) = (pixel::W * scale, pixel::CANVAS_H * scale);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            match px[y / scale][x / scale] {
                Some((r, g, b)) => rgba.extend_from_slice(&[r, g, b, 255]),
                None => rgba.extend_from_slice(&[0, 0, 0, 0]),
            }
        }
    }
    let data = base64(&rgba);
    let mut out = Vec::new();
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(4096).collect();
    for (i, c) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let head = if i == 0 { format!("a=T,f=32,s={w},v={h},c={cols},r={rows},C=1,q=2,m={more}") } else { format!("m={more}") };
        out.extend_from_slice(format!("\x1b_G{head};").as_bytes());
        out.extend_from_slice(c);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::{Extras, Eyes};

    fn px() -> Canvas {
        let s = &pixel::SPECIES[0];
        pixel::canvas(s, &s.palette, Pose { eyes: Eyes::Open, dy: 1, extras: Extras::default() })
    }

    /// Writes a sixel of every pet for an external decoder check:
    /// `COSMOS_SIXEL_OUT=<dir> cargo test -p ztui dump_sixels -- --ignored`.
    #[test]
    #[ignore]
    fn dump_sixels() {
        let dir = std::path::PathBuf::from(std::env::var("COSMOS_SIXEL_OUT").unwrap());
        for s in pixel::SPECIES {
            let px = pixel::canvas(s, &s.palette, Pose { eyes: Eyes::Open, dy: 1, extras: Extras { blush: true, sparkle: true, crown: true, ..Default::default() } });
            std::fs::write(dir.join(format!("{}.six", s.name)), sixel(&px, 6)).unwrap();
        }
    }

    #[test]
    fn sixel_is_well_formed() {
        let out = sixel(&px(), 4);
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1bP0;1;0q\"1;1;64;40") && s.ends_with("\x1b\\"), "{}", &s[..40]);
        assert!(s.contains("#0;2;"));
        assert_eq!(s.matches('-').count(), 40usize.div_ceil(6), "one band per six pixel rows");
        // every data byte is a sixel char or control
        let body = &s[s.find("\"1;1").unwrap()..s.len() - 2];
        assert!(body.bytes().all(|b| (63..=126).contains(&b) || b"#;!$-\"0123456789".contains(&b)), "{body}");
    }

    #[test]
    fn kitty_chunks_rgba() {
        let out = String::from_utf8(kitty(&px(), 8, 16, 5)).unwrap();
        assert!(out.starts_with("\x1b_Ga=T,f=32,s=128,v=80,c=16,r=5,C=1,q=2,m=1;"));
        assert!(out.ends_with("\x1b\\"));
        assert!(out.contains("\x1b_Gm=0;"), "the last chunk says so");
    }

    #[test]
    fn blocks_register_and_draw_in_place() {
        let mut cfg = BuddyCfg { images: "sixel".into(), ..Default::default() };
        let id = crate::buddy::Identity { species: 2, name: "x".into(), rarity: crate::buddy::Rarity::Common, stats: [1, 2, 3] };
        let b = block(&id, &cfg, Pose { eyes: Eyes::Open, dy: 1, extras: Extras::default() }, 6, 3).unwrap();
        assert!(b.rows >= 5 && b.cols >= 12);
        let line = b.mark(Line::from("hello"));
        let first = line.spans[0].content.to_string();
        let mut it = first.chars().peekable();
        let c = it.next().unwrap();
        let url = crate::markdown::take_link(c, &mut it).unwrap();
        let bytes = draw(&url).unwrap();
        assert!(bytes.starts_with(b"\x1b7\x1b[") && bytes.ends_with(b"\x1b8"));
        assert!(crate::wrap::width_of(&first) == 0, "the marker takes no columns");
        cfg.images = "off".into();
        assert!(block(&id, &cfg, Pose { eyes: Eyes::Open, dy: 1, extras: Extras::default() }, 6, 3).is_none());
    }
}
