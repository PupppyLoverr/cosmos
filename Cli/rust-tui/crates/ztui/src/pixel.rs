//! Pixel-art pets: 16×9 sprites, drawn solid with half-block characters
//! (`▀` / `▄`, one cell = two stacked pixels in true colour), and turned
//! into real images (sixel / kitty) where the terminal can show them.
//!
//! Each sprite is a grid of palette letters, so one drawing serves every
//! colourway: `o` outline, `b` body, `d` body shade, `l` belly / light,
//! `a` accent, `p` cheek, `k` dark details, `w` white, `y` gold, `e` eye.
//! Eyes are 2×2 clusters, redrawn for the mood (shiny, closed, happy `^`,
//! hearts, …), so a pet visibly reacts without extra drawings.

use ratatui_core::style::{Color, Style};
use ratatui_core::text::Span;

pub const W: usize = 16;
/// Sprite rows; the canvas is one taller so the pet can hop.
pub const H: usize = 9;
pub const CANVAS_H: usize = H + 1;

/// One species: name, a one-line description, its drawing and colours.
pub struct Species {
    pub name: &'static str,
    pub blurb: &'static str,
    pub art: [&'static str; H],
    pub palette: Palette,
}

/// RGB per palette letter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub o: (u8, u8, u8),
    pub b: (u8, u8, u8),
    pub d: (u8, u8, u8),
    pub l: (u8, u8, u8),
    pub a: (u8, u8, u8),
    pub p: (u8, u8, u8),
    pub k: (u8, u8, u8),
    /// eye colour when open (most pets: near-black)
    pub e: (u8, u8, u8),
}

#[allow(clippy::too_many_arguments)] // one per palette letter
const fn pal(o: u32, b: u32, d: u32, l: u32, a: u32, p: u32, k: u32, e: u32) -> Palette {
    const fn c(x: u32) -> (u8, u8, u8) {
        ((x >> 16) as u8, (x >> 8) as u8, x as u8)
    }
    Palette { o: c(o), b: c(b), d: c(d), l: c(l), a: c(a), p: c(p), k: c(k), e: c(e) }
}

pub const SPECIES: &[Species] = &[
    Species {
        name: "fox",
        blurb: "quick, curious, a little smug",
        art: [
            "..oo........oo..",
            ".oaao......oaao.",
            ".obbboooooobbbo.",
            ".obbbbbbbbbbbbo.",
            ".obeebbbbbbeebo.",
            ".obeebllllbeebo.",
            ".opllllkkllllpo.",
            "..ollllllllllo..",
            "...oo.oooo.oo...",
        ],
        palette: pal(0x3a1d0e, 0xf08a3c, 0xc9662a, 0xfff1dc, 0x2a1408, 0xff9fae, 0x2a1408, 0x1c1020),
    },
    Species {
        name: "cat",
        blurb: "calm, picky, secretly proud of you",
        art: [
            ".oo..........oo.",
            ".oao........oao.",
            ".obboooooooobbo.",
            ".obbbbbbbbbbbbo.",
            ".obeebbbbbbeebo.",
            "wobeebbppbbeebow",
            "wobbbbbkkbbbbbow",
            "..obbllllllbbo..",
            "...oo.oooo.oo...",
        ],
        palette: pal(0x2b2440, 0xa79bd6, 0x8476b8, 0xe9e3ff, 0xff9fd0, 0xff9fd0, 0x3d2f5c, 0x1a1530),
    },
    Species {
        name: "axolotl",
        blurb: "soft, cheerful, regrows after every bug",
        art: [
            "................",
            "a.a.oooooooo.a.a",
            ".aaobbbbbbbboaa.",
            "aa.obbbbbbbbo.aa",
            ".aaobeebbeeboaa.",
            "...obeebbeebo...",
            "...opbbkkbbpo...",
            "...obllllllbo...",
            "....oo....oo....",
        ],
        palette: pal(0x6b2a4a, 0xffb3d1, 0xf28fb8, 0xffe3ee, 0xff5fa2, 0xff6f9f, 0x7a2b52, 0x2a0f1e),
    },
    Species {
        name: "owl",
        blurb: "wise, nocturnal, reads every diff",
        art: [
            ".o............o.",
            ".oo..........oo.",
            ".obboooooooobbo.",
            ".obwwwwbbwwwwbo.",
            ".obweewbbweewbo.",
            ".obweewaaweewbo.",
            ".obbwwbaabwwbbo.",
            "..obllllllllbo..",
            "...oaa....aao...",
        ],
        palette: pal(0x2e1c10, 0x9a6b45, 0x7a5233, 0xe8cfa8, 0xf2b631, 0xd98c6a, 0x2e1c10, 0x1a1008),
    },
    Species {
        name: "penguin",
        blurb: "loyal, tidy, thrives in cold builds",
        art: [
            "................",
            "....oooooooo....",
            "...obbbbbbbbo...",
            "..obbllbbllbbo..",
            "..obleelleelbo..",
            "..obleelleelbo..",
            ".obbllaaaallbbo.",
            ".obbllllllllbbo.",
            "...aaa....aaa...",
        ],
        palette: pal(0x0e1424, 0x24304f, 0x18213a, 0xf4f7ff, 0xffa53d, 0xff9fae, 0x0e1424, 0x0e1424),
    },
    Species {
        name: "dragon",
        blurb: "bold, fiery, hoards passing tests",
        art: [
            "..a..........a..",
            "..ao........oa..",
            "..obboooooobbo..",
            ".obbbbbbbbbbbbo.",
            "dobeebbbbbbeebod",
            "dobeebllllbeebod",
            ".obbllllkklllbbo",
            "..obllllllllbo..",
            "...oo.oooo.oo.dd",
        ],
        palette: pal(0x123a22, 0x4cc46e, 0x2f8f4c, 0xd8f5c4, 0xffd166, 0xff9fae, 0x123a22, 0x10200f),
    },
    Species {
        name: "slime",
        blurb: "bouncy, simple, absorbs your stress",
        art: [
            "................",
            "................",
            "......oooo......",
            "....oobbbboo....",
            "...obwbbbbbbo...",
            "..obbeebbeebbo..",
            "..obbeebbeebbo..",
            ".obbbbpkkpbbbbo.",
            ".oooooooooooooo.",
        ],
        palette: pal(0x0f4652, 0x5fe0e6, 0x34b8c4, 0xc8fbff, 0xffffff, 0xff9fd0, 0x0f4652, 0x0b2a33),
    },
    Species {
        name: "ghost",
        blurb: "shy, floaty, haunts your TODOs",
        art: [
            "................",
            ".....oooooo.....",
            "....obbbbbbo....",
            "...obbbbbbbbo...",
            "...obeebbeebo...",
            "...obeebbeebo...",
            "..obbbpkkpbbbo..",
            "..obbbbbbbbbbo..",
            "..obo.obbo.obo..",
        ],
        palette: pal(0x3b3560, 0xf1edff, 0xcfc8f5, 0xffffff, 0xa79bd6, 0xffb3d1, 0x3b3560, 0x22203a),
    },
    Species {
        name: "octopus",
        blurb: "clever, eight hands for eight tasks",
        art: [
            "................",
            "....oooooooo....",
            "...obbbbbbbbo...",
            "..obbbbbbbbbbo..",
            "..obbeebbeebbo..",
            "..obbeebbeebbo..",
            "..obbbpkkpbbbo..",
            "obobobobbobobobo",
            "o.b.o.b..b.o.b.o",
        ],
        palette: pal(0x3a1554, 0xb36bff, 0x8c45d9, 0xe2c7ff, 0xffd166, 0xff8fc7, 0x3a1554, 0x1e0b2c),
    },
    Species {
        name: "robot",
        blurb: "precise, tireless, beeps at red tests",
        art: [
            ".......aa.......",
            ".......oo.......",
            "..oooooooooooo..",
            "..obbbbbbbbbbo..",
            "..obddddddddbo..",
            "..obdeeddeedbo..",
            "..obdeeddeedbo..",
            "..obbbbbbbbbbo..",
            "...oo......oo...",
        ],
        palette: pal(0x1f2733, 0xb8c4d6, 0x2a3442, 0xe6edf7, 0xff4d6d, 0xff9fae, 0x1f2733, 0x5fe0ff),
    },
];

/// Old species names (before pixel pets) → today's.
pub fn canonical(name: &str) -> Option<&'static Species> {
    let n = name.trim().to_ascii_lowercase();
    let n = match n.as_str() {
        "astrocat" => "cat",
        "jellyfish" | "comet" | "star" => "slime",
        "moth" => "owl",
        "alien" => "ghost",
        "snail" => "axolotl",
        other => other,
    };
    SPECIES.iter().find(|s| s.name == n)
}

/// How the eyes look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Eyes {
    Open,
    Focused,
    Closed,
    Happy,
    Up,
    Hearts,
}

/// Extras drawn over the pet (unlocked by level, or moods).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extras {
    pub blush: bool,
    pub sparkle: bool,
    pub crown: bool,
    pub sweat: bool,
    pub shiny: bool,
}

/// What to draw: which pet, how it feels, where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub eyes: Eyes,
    /// 1 = resting on the ground, 0 = mid-hop (a shadow shows below)
    pub dy: usize,
    pub extras: Extras,
}

pub type Rgb = (u8, u8, u8);

/// Shift a colour's hue for shiny pets (rotate the channels).
fn shiny(c: Rgb) -> Rgb {
    (c.2, c.0, c.1)
}

/// The pet as a 16×10 grid of colours (None = transparent).
pub fn canvas(sp: &Species, palette: &Palette, pose: Pose) -> [[Option<Rgb>; W]; CANVAS_H] {
    let mut px = [[None; W]; CANVAS_H];
    let white = (255, 255, 255);
    let gold = (255, 209, 102);
    let mut eye_cells: Vec<(usize, usize)> = Vec::new();
    for (y, row) in sp.art.iter().enumerate() {
        for (x, ch) in row.chars().enumerate().take(W) {
            let c = match ch {
                'o' => palette.o,
                'b' => palette.b,
                'd' => palette.d,
                'l' => palette.l,
                'a' => palette.a,
                'p' => {
                    if pose.extras.blush {
                        palette.p
                    } else {
                        palette.b
                    }
                }
                'k' => palette.k,
                'w' => white,
                'y' => gold,
                'e' => {
                    eye_cells.push((x, y));
                    palette.e
                }
                _ => continue,
            };
            let c = if pose.extras.shiny && ch != 'w' && ch != 'e' { shiny(c) } else { c };
            px[y + pose.dy][x] = Some(c);
        }
    }
    // eyes: each 2×2 cluster, top-left first
    for &(x, y) in &eye_cells {
        let left = !eye_cells.contains(&(x.wrapping_sub(1), y));
        let top = !eye_cells.contains(&(x, y.wrapping_sub(1)));
        let body = if pose.extras.shiny { shiny(palette.b) } else { palette.b };
        let c = match pose.eyes {
            Eyes::Open => {
                if top && left {
                    white
                } else {
                    palette.e
                }
            }
            Eyes::Focused => palette.e,
            Eyes::Closed => {
                if top {
                    body
                } else {
                    palette.o
                }
            }
            Eyes::Happy => {
                if top {
                    palette.o
                } else {
                    body
                }
            }
            Eyes::Up => {
                if top {
                    palette.e
                } else {
                    white
                }
            }
            Eyes::Hearts => (255, 77, 125),
        };
        px[y + pose.dy][x] = Some(c);
    }
    if pose.extras.crown && pose.dy == 1 {
        px[0][6..10].fill(Some(gold));
        px[0][6] = Some((255, 77, 109));
        px[0][9] = Some((95, 224, 230));
    }
    if pose.extras.sparkle {
        let (x, y) = (W - 1, pose.dy);
        px[y][x] = Some(gold);
    }
    if pose.extras.sweat {
        px[2 + pose.dy][W - 2] = Some((120, 200, 255));
    }
    if pose.dy == 0 {
        // a soft shadow under a hopping pet
        px[CANVAS_H - 1][5..11].fill(Some((60, 60, 72)));
    }
    px
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Half-block rows: each cell shows two stacked pixels; `scale` widens
/// every pixel to that many columns (and as many pixel rows).
pub fn half_blocks(px: &[[Option<Rgb>; W]; CANVAS_H], scale: usize) -> Vec<Vec<Span<'static>>> {
    let scale = scale.max(1);
    let h = CANVAS_H * scale;
    let at = |x: usize, y: usize| if y < h { px[y / scale][x / scale] } else { None };
    let mut rows = Vec::new();
    for r in 0..h.div_ceil(2) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for x in 0..W * scale {
            let (top, bot) = (at(x, 2 * r), at(x, 2 * r + 1));
            let (ch, st) = match (top, bot) {
                (None, None) => (' ', Style::new()),
                (Some(t), None) => ('▀', Style::new().fg(rgb(t))),
                (None, Some(b)) => ('▄', Style::new().fg(rgb(b))),
                (Some(t), Some(b)) if t == b => ('█', Style::new().fg(rgb(t))),
                (Some(t), Some(b)) => ('▀', Style::new().fg(rgb(t)).bg(rgb(b))),
            };
            match spans.last_mut() {
                Some(s) if s.style == st => {
                    let mut c = s.content.to_string();
                    c.push(ch);
                    s.content = c.into();
                }
                _ => spans.push(Span::styled(ch.to_string(), st)),
            }
        }
        rows.push(spans);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sprite_is_well_formed() {
        for s in SPECIES {
            for (i, r) in s.art.iter().enumerate() {
                assert_eq!(r.chars().count(), W, "{} row {i}: {r:?}", s.name);
                assert!(r.chars().all(|c| ".obdlapkwye".contains(c)), "{} row {i}", s.name);
            }
            // two eyes, each a 2×2 block
            let eyes: Vec<(usize, usize)> =
                s.art.iter().enumerate().flat_map(|(y, r)| r.chars().enumerate().filter(|(_, c)| *c == 'e').map(move |(x, _)| (x, y))).collect();
            assert_eq!(eyes.len(), 8, "{}", s.name);
            let tops: Vec<_> = eyes.iter().filter(|(x, y)| !eyes.contains(&(*x, y.wrapping_sub(1))) && !eyes.contains(&(x.wrapping_sub(1), *y))).collect();
            assert_eq!(tops.len(), 2, "{}: {tops:?}", s.name);
            // left/right symmetric silhouette (detail colours may differ)
            for r in s.art {
                let solid: Vec<bool> = r.chars().map(|c| c != '.').collect();
                let mirrored: Vec<bool> = solid.iter().rev().copied().collect();
                let diff = solid.iter().zip(&mirrored).filter(|(a, b)| a != b).count();
                assert!(diff <= 4, "{} is lopsided: {r:?}", s.name);
            }
        }
    }

    #[test]
    fn moods_change_the_eyes_and_hops_cast_a_shadow() {
        let s = &SPECIES[0];
        let open = canvas(s, &s.palette, Pose { eyes: Eyes::Open, dy: 1, extras: Extras::default() });
        let closed = canvas(s, &s.palette, Pose { eyes: Eyes::Closed, dy: 1, extras: Extras::default() });
        assert_ne!(open, closed);
        assert!(open[CANVAS_H - 1].iter().any(|p| p.is_some()), "resting: feet on the ground row");
        let hop = canvas(s, &s.palette, Pose { eyes: Eyes::Happy, dy: 0, extras: Extras::default() });
        assert_eq!(hop[CANVAS_H - 1][7], Some((60, 60, 72)), "shadow under the hop");
        let crowned = canvas(s, &s.palette, Pose { eyes: Eyes::Open, dy: 1, extras: Extras { crown: true, ..Default::default() } });
        assert!(crowned[0][7].is_some());
    }

    #[test]
    fn half_blocks_cover_the_canvas() {
        let s = &SPECIES[3];
        let px = canvas(s, &s.palette, Pose { eyes: Eyes::Open, dy: 1, extras: Extras::default() });
        let rows = half_blocks(&px, 1);
        assert_eq!(rows.len(), 5);
        for r in &rows {
            let w: usize = r.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(w, W);
        }
        assert_eq!(half_blocks(&px, 2).len(), 10);
        assert!(canonical("astrocat").is_some_and(|s| s.name == "cat"));
        assert!(canonical("Dragon").is_some());
    }
}
