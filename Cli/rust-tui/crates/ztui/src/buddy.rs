//! Your buddy: a small cosmic creature that sits beside the composer,
//! watches the task, and reacts — focused while a turn runs, pleased when it
//! lands, worried by errors and red tests, asleep when you wander off.
//!
//! It is hatched deterministically from your name (plus `[buddy] seed`), so
//! the same person always gets the same buddy until they `/buddy reroll`.
//! Quips are canned one-liners (skinnable in `[buddy.lines]`): no model
//! calls, no tokens.

use std::time::{Duration, Instant};

use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::{Line, Span};

use crate::skin::BuddyCfg;

/// (species, three sprite rows). `@` marks the eyes (left, then right).
pub const SPECIES: &[(&str, [&str; 3])] = &[
    ("comet", ["  ✦ ~~~ ", " ( @ @ )", "  `‿´  "]),
    ("octopus", ["  .---. ", " ( @ @ )", " /|/|\\|\\"]),
    ("astrocat", ["  /\\_/\\ ", " ( @ @ )", "  > ^ < "]),
    ("owl", ["  ,___, ", "  (@v@) ", "  /)_)  "]),
    ("axolotl", ["}(  ‿ ){", " ( @ @ )", "  ∪   ∪ "]),
    ("robot", ["  [‾‾‾] ", "  [@_@] ", "  /|_|\\ "]),
    ("jellyfish", ["  .-~-. ", " ( @ @ )", "  )|(|( "]),
    ("fox", [" /\\   /\\", " ( @ @ )", "  \\ ▾ / "]),
    ("ghost", ["  .-.   ", " (@ @)  ", " |/\\/|  "]),
    ("moth", [" \\(   )/", " /(@ @)\\", "   ' '  "]),
    ("penguin", ["  (@v@) ", " /(   )\\", "  ^^ ^^ "]),
    ("slime", ["   ___  ", " _(@ @)_", "(_______)"]),
    ("dragon", ["  /\\_/\\_", " ( @ @  >", "  \\_^^_/ "]),
    ("alien", ["  \\  /  ", " (@  @) ", "  ) ‿ ( "]),
    ("snail", ["  @ @   ", "  )_)__ ", " (_____)"]),
    ("star", ["   /\\   ", " <(@ @)>", "   \\/   "]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Cosmic,
}

impl Rarity {
    pub fn label(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Uncommon => "uncommon",
            Rarity::Rare => "rare",
            Rarity::Epic => "epic",
            Rarity::Cosmic => "cosmic",
        }
    }

    fn color(self) -> Color {
        let p = crate::skin::palette();
        match self {
            Rarity::Common => p.text,
            Rarity::Uncommon => p.success,
            Rarity::Rare => p.link,
            Rarity::Epic => p.accent,
            Rarity::Cosmic => p.warn,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mood {
    Idle,
    Focused,
    Happy,
    Worried,
    Waiting,
    Sleeping,
    Loved,
}

impl Mood {
    pub fn label(self) -> &'static str {
        match self {
            Mood::Idle => "idle",
            Mood::Focused => "focused",
            Mood::Happy => "pleased",
            Mood::Worried => "worried",
            Mood::Waiting => "waiting on you",
            Mood::Sleeping => "asleep",
            Mood::Loved => "loved",
        }
    }

    fn eyes(self) -> (char, char) {
        match self {
            Mood::Idle => ('•', '•'),
            Mood::Focused => ('◉', '◉'),
            Mood::Happy => ('^', '^'),
            Mood::Worried => ('°', '°'),
            Mood::Waiting => ('•', '•'),
            Mood::Sleeping => ('-', '-'),
            Mood::Loved => ('♥', '♥'),
        }
    }
}

/// Who your buddy is (fixed for a given name + seed).
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub species: usize,
    pub name: String,
    pub rarity: Rarity,
    /// focus, chaos, snark — 0..=100
    pub stats: [u8; 3],
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A well-mixed 64-bit step (splitmix64).
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

const FIRST: &[&str] = &["Zo", "Ka", "Lu", "Mi", "No", "Pi", "Ra", "Su", "Te", "Vy", "Xa", "Quo", "Bea", "Io", "Ny", "Ori"];
const LAST: &[&str] = &["by", "mo", "ra", "lix", "zel", "nova", "pip", "tor", "wen", "ix", "bit", "sol", "kin", "dle", "mi", "zar"];

/// Hatch the buddy for `who` (the operator's name) and the config.
pub fn hatch(who: &str, cfg: &BuddyCfg) -> Identity {
    let seed = mix(fnv(who) ^ mix(cfg.seed));
    let r = mix(seed) % 100;
    let rarity = match r {
        0 => Rarity::Cosmic,
        1..=4 => Rarity::Epic,
        5..=14 => Rarity::Rare,
        15..=39 => Rarity::Uncommon,
        _ => Rarity::Common,
    };
    let species = SPECIES
        .iter()
        .position(|(n, _)| n.eq_ignore_ascii_case(cfg.species.trim()))
        .unwrap_or((mix(seed ^ 1) % SPECIES.len() as u64) as usize);
    let floor = rarity as u64 * 10;
    let stat = |k: u64| (floor + mix(seed ^ k) % (101 - floor)) as u8;
    let name = if cfg.name.trim().is_empty() {
        let a = FIRST[(mix(seed ^ 7) % FIRST.len() as u64) as usize];
        let b = LAST[(mix(seed ^ 8) % LAST.len() as u64) as usize];
        format!("{a}{b}")
    } else {
        cfg.name.trim().to_string()
    };
    Identity { species, name, rarity, stats: [stat(2), stat(3), stat(4)] }
}

/// The moments a buddy reacts to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Moment {
    Hello,
    TurnStart,
    Done { secs: u64 },
    Error,
    /// one tool call failed — a frown, no comment
    ToolError,
    TestsPass,
    TestsFail,
    Waiting,
    Answered,
    Compact,
    Degraded,
    Pet,
    Input,
}

fn builtin(kind: &str) -> &'static [&'static str] {
    match kind {
        "hello" => &["{name} is here. Let's build something.", "Hi! {name}, reporting for duty.", "Ready when you are."],
        "done" => &["Done!", "That went smoothly.", "Another one lands.", "Nice."],
        "long_done" => &["Phew — that was a big one.", "All done. Stretch your legs?", "Finished! That took a while.", "Landed it."],
        "error" => &["Hmm, that didn't work.", "Ouch. Let's look at that error.", "Something broke — not your fault. Probably."],
        "tests_pass" => &["All green!", "Tests pass. Ship it?", "Green across the board."],
        "tests_fail" => &["Red tests. We've got this.", "A few failures to chase.", "The tests disagree."],
        "waiting" => &["Need your OK on this one.", "Waiting on you.", "Your call."],
        "compact" => &["Tidied up the context.", "Packed the memory a bit tighter."],
        "pet" => &["♥", "{name} purrs.", "Happy {name} noises.", "(˘‿˘)"],
        "sleep" => &["zZz", "Dreaming of clean builds…"],
        "degraded" => &["The sandbox is weaker than usual — careful.", "Heads up: the sandbox is degraded."],
        _ => &[],
    }
}

#[derive(Default)]
pub struct Buddy {
    /// mood overrides with an expiry
    happy_until: Option<Instant>,
    worried_until: Option<Instant>,
    loved_until: Option<Instant>,
    waiting: bool,
    last_input: Option<Instant>,
    /// (text, shown at)
    pub quip: Option<(String, Instant)>,
    quips_said: u64,
    /// the last test run failed (so a pass is worth celebrating)
    red: bool,
}

impl Buddy {
    pub fn new() -> Buddy {
        Buddy { last_input: Some(Instant::now()), ..Default::default() }
    }

    fn say(&mut self, kind: &str, cfg: &BuddyCfg, name: &str) {
        if cfg.quips == "off" {
            return;
        }
        let own = cfg.lines.get(kind).filter(|l| !l.is_empty());
        let n = own.map(|l| l.len()).unwrap_or_else(|| builtin(kind).len());
        if n == 0 {
            return;
        }
        let i = (mix(self.quips_said ^ fnv(kind)) % n as u64) as usize;
        let line = own.map(|l| l[i].clone()).unwrap_or_else(|| builtin(kind)[i].to_string());
        self.quips_said += 1;
        self.quip = Some((line.replace("{name}", name), Instant::now()));
    }

    /// React to something that happened.
    pub fn on(&mut self, m: Moment, cfg: &BuddyCfg, name: &str) {
        let now = Instant::now();
        let secs = |s: u64| Some(now + Duration::from_secs(s));
        match m {
            Moment::Hello => self.say("hello", cfg, name),
            Moment::TurnStart => {
                self.quip = None;
                self.last_input = Some(now);
            }
            Moment::Done { secs: s } => {
                self.happy_until = secs(6);
                if s >= 60 {
                    self.say("long_done", cfg, name);
                } else if s >= 20 {
                    self.say("done", cfg, name);
                }
            }
            Moment::Error => {
                self.worried_until = secs(8);
                self.say("error", cfg, name);
            }
            Moment::ToolError => self.worried_until = secs(5),
            Moment::TestsPass => {
                self.happy_until = secs(8);
                if std::mem::take(&mut self.red) {
                    self.say("tests_pass", cfg, name);
                }
            }
            Moment::TestsFail => {
                self.worried_until = secs(8);
                if !std::mem::replace(&mut self.red, true) {
                    self.say("tests_fail", cfg, name);
                }
            }
            Moment::Waiting => {
                self.waiting = true;
                self.say("waiting", cfg, name);
            }
            Moment::Answered => {
                self.waiting = false;
                self.quip = None;
            }
            Moment::Compact => self.say("compact", cfg, name),
            Moment::Degraded => {
                self.worried_until = secs(10);
                self.say("degraded", cfg, name);
            }
            Moment::Pet => {
                self.loved_until = secs(3);
                self.say("pet", cfg, name);
            }
            Moment::Input => self.last_input = Some(now),
        }
    }

    pub fn mood(&self, busy: bool) -> Mood {
        let now = Instant::now();
        let live = |t: Option<Instant>| t.is_some_and(|t| t > now);
        if live(self.loved_until) {
            Mood::Loved
        } else if self.waiting {
            Mood::Waiting
        } else if live(self.worried_until) {
            Mood::Worried
        } else if live(self.happy_until) {
            Mood::Happy
        } else if busy {
            Mood::Focused
        } else if self.last_input.is_some_and(|t| t.elapsed() > Duration::from_secs(300)) {
            Mood::Sleeping
        } else {
            Mood::Idle
        }
    }

    /// The quip still on screen, if any.
    pub fn current_quip(&self, cfg: &BuddyCfg) -> Option<&str> {
        self.quip
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(cfg.quip_secs.max(1)))
            .map(|(q, _)| q.as_str())
    }
}

/// Columns a sprite takes (all rows padded to this).
pub fn sprite_width(id: &Identity) -> usize {
    SPECIES[id.species].1.iter().map(|r| crate::wrap::width_of(r)).max().unwrap_or(8)
}

/// Blink for 150 ms every ~4 s while idle or focused.
fn blinking(mood: Mood, ms: u128) -> bool {
    matches!(mood, Mood::Idle | Mood::Focused) && ms % 4200 < 150
}

/// The three sprite rows for `mood`, coloured by rarity.
pub fn sprite(id: &Identity, mood: Mood, ms: u128, reduce: bool) -> Vec<Vec<Span<'static>>> {
    let (l, r) = if !reduce && blinking(mood, ms) { ('-', '-') } else { mood.eyes() };
    let width = sprite_width(id);
    let body = Style::new().fg(id.rarity.color());
    let eye = Style::new().fg(crate::skin::palette().text).add_modifier(Modifier::BOLD);
    let mut left_eye = true;
    SPECIES[id.species]
        .1
        .iter()
        .map(|row| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut buf = String::new();
            for c in row.chars() {
                if c == '@' {
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut buf), body));
                    }
                    spans.push(Span::styled((if left_eye { l } else { r }).to_string(), eye));
                    left_eye = !left_eye;
                } else {
                    buf.push(c);
                }
            }
            let pad = width.saturating_sub(crate::wrap::width_of(row));
            buf.push_str(&" ".repeat(pad));
            spans.push(Span::styled(buf, body));
            if id.rarity == Rarity::Cosmic && !reduce {
                spans = spans
                    .into_iter()
                    .flat_map(|s| if s.style == body { crate::motion::shimmer(&s.content, id.rarity.color(), ms, true) } else { vec![s] })
                    .collect();
            }
            spans
        })
        .collect()
}

/// A one-glyph-wide face for the status line: `(•ᴥ•)`.
pub fn mini(id: &Identity, mood: Mood) -> Span<'static> {
    let (l, r) = mood.eyes();
    Span::styled(format!("({l}ᴥ{r})"), Style::new().fg(id.rarity.color()))
}

/// The `/buddy` card.
pub fn card(id: &Identity, mood: Mood) -> Vec<Line<'static>> {
    let dim = |s: String| Span::styled(s, crate::theme::DIM());
    let bar = |v: u8| {
        let n = (v as usize + 5) / 10;
        format!("{}{} {v:>3}", "█".repeat(n), "░".repeat(10 - n))
    };
    let info = [
        vec![
            Span::styled(id.name.clone(), Style::new().fg(id.rarity.color()).add_modifier(Modifier::BOLD)),
            dim(format!("  the {} {}", id.rarity.label(), SPECIES[id.species].0)),
        ],
        vec![dim(format!("focus  {}", bar(id.stats[0])))],
        vec![dim(format!("chaos  {}", bar(id.stats[1])))],
        vec![dim(format!("snark  {}", bar(id.stats[2])))],
        vec![dim(format!("feeling {}", mood.label()))],
    ];
    let rows = sprite(id, mood, 1000, true);
    let w = sprite_width(id);
    let mut out = Vec::new();
    for (i, info) in info.into_iter().enumerate() {
        let mut spans = vec![Span::raw(" ")];
        match rows.get(i) {
            Some(r) => spans.extend(r.iter().cloned()),
            None => spans.push(Span::raw(" ".repeat(w))),
        }
        spans.push(Span::raw("   "));
        spans.extend(info);
        out.push(Line::from(spans));
    }
    out.push(Line::from(dim(" /buddy pet · name <name> · reroll · mini · full · quiet · off".into())));
    out
}

/// What a bash call tells us about tests: Some(true) pass, Some(false) fail.
pub fn test_outcome(command: &str, content: &str) -> Option<bool> {
    let c = command.to_ascii_lowercase();
    let is_test = ["test", "pytest", "jest", "vitest", "mocha", "rspec", "ctest", "check"].iter().any(|k| c.contains(k));
    if !is_test {
        return None;
    }
    let (code, _, _) = crate::toolview::shell_parts(content);
    match code.as_deref() {
        Some("0") => Some(true),
        Some("running") | None => None,
        Some(_) => Some(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hatching_is_stable_and_seeded() {
        let cfg = BuddyCfg::default();
        let a = hatch("ravth", &cfg);
        assert_eq!(a, hatch("ravth", &cfg), "same name, same buddy");
        let mut other = cfg.clone();
        other.seed = 1;
        let b = hatch("ravth", &other);
        assert!(a != b, "a reroll changes something");
        other.species = "Fox".into();
        other.name = "Pip".into();
        let c = hatch("ravth", &other);
        assert_eq!((SPECIES[c.species].0, c.name.as_str()), ("fox", "Pip"));
        assert!(c.stats.iter().all(|s| *s <= 100));
    }

    #[test]
    fn every_sprite_has_two_eyes_and_fits() {
        for (i, (name, rows)) in SPECIES.iter().enumerate() {
            let eyes: usize = rows.iter().map(|r| r.matches('@').count()).sum();
            assert_eq!(eyes, 2, "{name}");
            let id = Identity { species: i, name: "x".into(), rarity: Rarity::Common, stats: [1, 2, 3] };
            let w = sprite_width(&id);
            assert!(w <= 10, "{name} is {w} wide");
            for r in sprite(&id, Mood::Loved, 0, true) {
                let t: String = r.iter().map(|s| s.content.to_string()).collect();
                assert_eq!(crate::wrap::width_of(&t), w, "{name}: {t:?}");
                assert!(!t.contains('@'));
            }
        }
    }

    #[test]
    fn moods_follow_the_task() {
        let cfg = BuddyCfg::default();
        let mut b = Buddy::new();
        assert_eq!(b.mood(false), Mood::Idle);
        assert_eq!(b.mood(true), Mood::Focused);
        b.on(Moment::Waiting, &cfg, "Zo");
        assert_eq!(b.mood(true), Mood::Waiting);
        assert!(b.current_quip(&cfg).is_some());
        b.on(Moment::Answered, &cfg, "Zo");
        b.on(Moment::TestsFail, &cfg, "Zo");
        assert_eq!(b.mood(true), Mood::Worried);
        assert!(b.current_quip(&cfg).is_some_and(|q| builtin("tests_fail").contains(&q)));
        b.on(Moment::TestsPass, &cfg, "Zo");
        assert!(b.current_quip(&cfg).is_some_and(|q| builtin("tests_pass").contains(&q)), "green after red is news");
        b.on(Moment::Pet, &cfg, "Zo");
        assert_eq!(b.mood(false), Mood::Loved);
        // a short turn ends quietly; a long one gets a word
        let mut b = Buddy::new();
        b.on(Moment::Done { secs: 3 }, &cfg, "Zo");
        assert!(b.quip.is_none());
        assert_eq!(b.mood(false), Mood::Happy);
        b.on(Moment::Done { secs: 90 }, &cfg, "Zo");
        assert!(b.quip.is_some());
        // quiet means quiet; custom lines win
        let mut quiet = cfg.clone();
        quiet.quips = "off".into();
        let mut b = Buddy::new();
        b.on(Moment::Error, &quiet, "Zo");
        assert!(b.quip.is_none());
        let mut own = cfg.clone();
        own.lines.insert("error".into(), vec!["{name} says: oops".into()]);
        b.on(Moment::Error, &own, "Zo");
        assert_eq!(b.current_quip(&own), Some("Zo says: oops"));
    }

    #[test]
    fn test_runs_are_recognised() {
        let ok = "exit_code=0 log_path=x omitted_chars=0\n<<<UNTRUSTED_SHELL\nok\n>>>";
        let bad = "exit_code=101 log_path=x omitted_chars=0\n<<<UNTRUSTED_SHELL\nFAILED\n>>>";
        assert_eq!(test_outcome("cargo test -p ztui", ok), Some(true));
        assert_eq!(test_outcome("npm test", bad), Some(false));
        assert_eq!(test_outcome("ls", ok), None);
    }
}
