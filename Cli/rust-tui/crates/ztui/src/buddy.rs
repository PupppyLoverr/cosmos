//! Your pet: a small pixel-art creature that sits beside the composer,
//! watches the task and grows with you.
//!
//! - **Looks**: solid pixel art (`pixel.rs`), real images where the terminal
//!   supports them (`image.rs`). Eyes, hops and extras follow its mood.
//! - **Watches**: focused while a turn runs, hops when it lands, frowns at
//!   failed tools, worries at red tests and cheers when they turn green,
//!   waits with you at permission prompts, dozes when you're away.
//! - **Grows**: finished turns, green tests, chats and daily streaks earn XP
//!   (`pet.json`); levels unlock blush, a sparkle and a crown.
//! - **Helps**: nudges you to take a break, tells you what happened while you
//!   were away, keeps reminders, and talks back with `/pettalk`.
//!
//! Hatched deterministically from your name (plus `[buddy] seed`), so you
//! keep the same pet until you `/buddy reroll` or pick one.

use std::path::Path;
use std::time::{Duration, Instant};

use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::{Line, Span};
use serde::{Deserialize, Serialize};

use crate::pixel::{self, Extras, Eyes, Pose, SPECIES};
use crate::skin::BuddyCfg;

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
    /// thinking about what you said (/pettalk)
    Listening,
}

impl Mood {
    pub fn label(self) -> &'static str {
        match self {
            Mood::Idle => "content",
            Mood::Focused => "focused",
            Mood::Happy => "delighted",
            Mood::Worried => "worried",
            Mood::Waiting => "waiting on you",
            Mood::Sleeping => "asleep",
            Mood::Loved => "loved",
            Mood::Listening => "listening",
        }
    }
}

/// Who your pet is (fixed for a given name + seed).
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub species: usize,
    pub name: String,
    pub rarity: Rarity,
    /// focus, chaos, snark — 0..=100
    pub stats: [u8; 3],
}

impl Identity {
    pub fn species(&self) -> &'static pixel::Species {
        &SPECIES[self.species]
    }

    /// A short personality from the stats, for /pettalk.
    pub fn personality(&self) -> String {
        let [focus, chaos, snark] = self.stats;
        let mut t = Vec::new();
        t.push(if focus > 66 { "laser-focused" } else if focus > 33 { "attentive" } else { "easily distracted" });
        t.push(if chaos > 66 { "chaotic" } else if chaos > 33 { "playful" } else { "orderly" });
        t.push(if snark > 66 { "very sarcastic" } else if snark > 33 { "a little cheeky" } else { "sweet" });
        format!("{}; {}", t.join(", "), self.species().blurb)
    }
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

/// Hatch the pet for `who` (the operator's name) and the config.
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
    let species = pixel::canonical(&cfg.species)
        .and_then(|s| SPECIES.iter().position(|x| x.name == s.name))
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

// ---- progress -----------------------------------------------------------------

/// What your pet has been through with you (`~/.cosmos-cli/pet.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub xp: u64,
    pub turns: u64,
    /// red tests turned green
    pub fixes: u64,
    pub chats: u64,
    pub streak: u32,
    pub best_streak: u32,
    /// UTC day number of the last activity
    pub last_day: u64,
}

/// Total XP needed to reach `level`.
pub fn xp_for(level: u32) -> u64 {
    let l = level.max(1) as u64;
    15 * (l - 1) * l / 2
}

/// Unlocks by level (shown on the card as "next").
pub const UNLOCKS: &[(u32, &str)] = &[(3, "rosy cheeks"), (5, "a ✦ sparkle"), (10, "a crown"), (15, "the shiny coat")];

fn today() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0)
}

impl Progress {
    pub fn path(home: &Path) -> std::path::PathBuf {
        home.join("pet.json")
    }

    pub fn load(home: &Path) -> Progress {
        std::fs::read_to_string(Self::path(home)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, home: &Path) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::create_dir_all(home);
            let _ = std::fs::write(Self::path(home), s);
        }
    }

    pub fn level(&self) -> u32 {
        let mut l = 1;
        while self.xp >= xp_for(l + 1) {
            l += 1;
        }
        l
    }

    /// Add XP (and count the day toward the streak). Some(level) on level-up.
    pub fn gain(&mut self, xp: u64) -> Option<u32> {
        self.gain_on(xp, today())
    }

    fn gain_on(&mut self, xp: u64, day: u64) -> Option<u32> {
        let before = self.level();
        if day != self.last_day {
            self.streak = if self.last_day + 1 == day { self.streak + 1 } else { 1 };
            self.best_streak = self.best_streak.max(self.streak);
            self.last_day = day;
            self.xp += 5 * u64::from(self.streak.min(7)); // a daily hello, worth more on a streak
        }
        self.xp += xp;
        let after = self.level();
        (after > before).then_some(after)
    }

    /// What the next unlock is, if any.
    pub fn next_unlock(&self) -> Option<(u32, &'static str)> {
        let l = self.level();
        UNLOCKS.iter().copied().find(|(at, _)| *at > l)
    }
}

// ---- moments and quips ----------------------------------------------------------

/// The moments a pet reacts to.
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
        "pet" => &["♥", "{name} wiggles happily.", "Happy {name} noises.", "{name} leans into it."],
        "sleep" => &["zZz", "Dreaming of clean builds…"],
        "degraded" => &["The sandbox is weaker than usual — careful.", "Heads up: the sandbox is degraded."],
        "break" => &["We've been at it a while — stretch break?", "{name} suggests water and a short walk.", "Eyes off the screen for a minute?"],
        "level" => &["{name} grew stronger!", "Level up! {name} looks proud.", "{name} learned something new."],
        _ => &[],
    }
}

/// A trick the pet can do on request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trick {
    Hop,
    Spin,
    Sparkle,
    Nap,
}

impl Trick {
    pub fn parse(s: &str) -> Option<Trick> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "hop" | "jump" | "bounce" => Trick::Hop,
            "spin" | "twirl" | "dance" => Trick::Spin,
            "sparkle" | "shine" | "glow" => Trick::Sparkle,
            "nap" | "sleep" => Trick::Nap,
            _ => return None,
        })
    }
}

#[derive(Default)]
pub struct Buddy {
    /// mood overrides with an expiry
    happy_until: Option<Instant>,
    worried_until: Option<Instant>,
    loved_until: Option<Instant>,
    trick: Option<(Trick, Instant)>,
    waiting: bool,
    /// /pettalk is waiting for an answer
    pub listening: bool,
    last_input: Option<Instant>,
    /// (text, shown at, seconds to keep it up)
    pub quip: Option<(String, Instant, u64)>,
    quips_said: u64,
    /// the last test run failed (so a pass is worth celebrating)
    red: bool,
    /// when the current stretch of work began (break reminders)
    pub working_since: Option<Instant>,
    pub progress: Progress,
    /// progress changed and should be saved
    pub progress_dirty: bool,
    /// a level reached since the UI last looked
    pub leveled: Option<u32>,
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
        self.quip = Some((line.replace("{name}", name), Instant::now(), cfg.quip_secs.max(1)));
    }

    /// Earn XP; remembers a level-up for the UI to announce.
    pub fn grant(&mut self, xp: u64) {
        if let Some(l) = self.progress.gain(xp) {
            self.leveled = Some(l);
        }
        self.progress_dirty = true;
    }

    /// A /pettalk exchange happened.
    pub fn chatted(&mut self) {
        self.progress.chats += 1;
        self.grant(2);
    }

    /// Show a line in the pet's bubble for `secs`.
    pub fn speak(&mut self, text: impl Into<String>, secs: u64) {
        self.quip = Some((text.into(), Instant::now(), secs.max(1)));
    }

    pub fn trick(&mut self, t: Trick) {
        self.trick = Some((t, Instant::now()));
        if t == Trick::Sparkle {
            self.loved_until = Some(Instant::now() + Duration::from_secs(2));
        }
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
                self.working_since.get_or_insert(now);
            }
            Moment::Done { secs: s } => {
                self.happy_until = secs(4);
                self.progress.turns += 1;
                self.grant(if s >= 60 { 5 } else { 3 });
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
                self.happy_until = secs(5);
                if std::mem::take(&mut self.red) {
                    self.progress.fixes += 1;
                    self.grant(10);
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
                self.grant(1);
                self.say("pet", cfg, name);
            }
            Moment::Input => {
                // a long gap ends a stretch of work
                if self.last_input.is_some_and(|t| t.elapsed() > Duration::from_secs(600)) {
                    self.working_since = None;
                }
                self.last_input = Some(now);
            }
        }
    }

    /// Say a canned line by kind (level-ups, break nudges).
    pub fn say_kind(&mut self, kind: &str, cfg: &BuddyCfg, name: &str) {
        self.say(kind, cfg, name);
    }

    /// True once per stretch when it's time to suggest a break.
    pub fn break_due(&mut self, mins: u64) -> bool {
        if mins == 0 {
            return false;
        }
        match self.working_since {
            Some(t) if t.elapsed() >= Duration::from_secs(mins * 60) => {
                self.working_since = Some(Instant::now());
                true
            }
            _ => false,
        }
    }

    pub fn mood(&self, busy: bool) -> Mood {
        let now = Instant::now();
        let live = |t: Option<Instant>| t.is_some_and(|t| t > now);
        if live(self.loved_until) {
            Mood::Loved
        } else if self.listening {
            Mood::Listening
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
    pub fn current_quip(&self, _cfg: &BuddyCfg) -> Option<&str> {
        self.quip.as_ref().filter(|(_, at, s)| at.elapsed() < Duration::from_secs(*s)).map(|(q, _, _)| q.as_str())
    }

    /// The trick in progress (they last ~1.6 s, a nap 4 s).
    fn active_trick(&self) -> Option<(Trick, u128)> {
        let (t, at) = self.trick?;
        let ms = at.elapsed().as_millis();
        let len = if t == Trick::Nap { 4000 } else { 1600 };
        (ms < len).then_some((t, ms))
    }

    /// How the pet looks right now.
    pub fn pose(&self, id: &Identity, busy: bool, ms: u128, reduce: bool) -> Pose {
        let mood = self.mood(busy);
        let level = self.progress.level();
        let blink = !reduce && matches!(mood, Mood::Idle | Mood::Focused | Mood::Listening) && ms % 4200 < 160;
        let mut eyes = match mood {
            Mood::Idle => Eyes::Open,
            Mood::Focused => Eyes::Focused,
            Mood::Happy => Eyes::Happy,
            Mood::Worried => Eyes::Open,
            Mood::Waiting | Mood::Listening => Eyes::Up,
            Mood::Sleeping => Eyes::Closed,
            Mood::Loved => Eyes::Hearts,
        };
        if blink {
            eyes = Eyes::Closed;
        }
        let hop = |period: u128, up: u128| !reduce && ms % period < up;
        let mut dy = match mood {
            Mood::Happy | Mood::Loved => usize::from(!hop(440, 220)),
            Mood::Waiting => usize::from(!hop(1400, 180)),
            Mood::Listening => usize::from(!hop(900, 150)),
            _ => 1,
        };
        let mut extras = Extras {
            blush: level >= 3 || mood == Mood::Loved,
            sparkle: level >= 5 || id.rarity >= Rarity::Epic,
            crown: level >= 10,
            sweat: mood == Mood::Worried,
            shiny: id.rarity == Rarity::Cosmic || level >= 15,
        };
        if let Some((t, tms)) = self.active_trick() {
            match t {
                Trick::Hop => {
                    eyes = Eyes::Happy;
                    dy = usize::from(!(tms % 400 < 200) || reduce);
                }
                Trick::Spin => {
                    // the eyes look around as it turns
                    eyes = [Eyes::Open, Eyes::Up, Eyes::Happy, Eyes::Focused][(tms / 200 % 4) as usize];
                    dy = usize::from(tms % 800 >= 400);
                }
                Trick::Sparkle => {
                    extras.sparkle = true;
                    extras.blush = true;
                    eyes = Eyes::Happy;
                }
                Trick::Nap => {
                    eyes = Eyes::Closed;
                    dy = 1;
                }
            }
        }
        Pose { eyes, dy, extras }
    }
}

// ---- drawing ------------------------------------------------------------------------

/// Columns the live sprite takes.
pub fn sprite_width(_id: &Identity) -> usize {
    pixel::W
}

/// The species' colours with any `[buddy.colors]` overrides.
pub fn palette_for(id: &Identity, cfg: &BuddyCfg) -> pixel::Palette {
    let mut p = id.species().palette;
    for (k, v) in &cfg.colors {
        let Some(Color::Rgb(r, g, b)) = crate::skin::palette::parse_color(v) else { continue };
        let slot = match k.as_str() {
            "outline" => &mut p.o,
            "body" => &mut p.b,
            "shade" => &mut p.d,
            "belly" => &mut p.l,
            "accent" => &mut p.a,
            "cheeks" => &mut p.p,
            "details" => &mut p.k,
            "eyes" => &mut p.e,
            _ => continue,
        };
        *slot = (r, g, b);
    }
    p
}

/// The pet as five rows of half-blocks (16 columns), for beside the composer.
pub fn sprite(id: &Identity, cfg: &BuddyCfg, pose: Pose) -> Vec<Vec<Span<'static>>> {
    let px = pixel::canvas(id.species(), &palette_for(id, cfg), pose);
    pixel::half_blocks(&px, 1)
}

/// A one-glyph face for the status line: `(•ᴥ•)` in the pet's colour.
pub fn mini(id: &Identity, mood: Mood) -> Span<'static> {
    let (l, r) = match mood {
        Mood::Idle | Mood::Worried => ('•', '•'),
        Mood::Focused => ('◉', '◉'),
        Mood::Happy => ('^', '^'),
        Mood::Waiting | Mood::Listening => ('°', '°'),
        Mood::Sleeping => ('-', '-'),
        Mood::Loved => ('♡', '♡'),
    };
    let (r0, g0, b0) = id.species().palette.b;
    Span::styled(format!("({l}ᴥ{r})"), Style::new().fg(Color::Rgb(r0, g0, b0)))
}

fn bar(v: u64, max: u64, n: usize) -> String {
    let fill = (v.min(max) * n as u64 + max / 2).checked_div(max).map_or(n, |f| f as usize);
    format!("{}{}", "█".repeat(fill), "░".repeat(n - fill))
}

/// The `/buddy` card: the pet large on the left (a real image where the
/// terminal can show one), who it is and how it's doing on the right.
pub fn card(id: &Identity, cfg: &BuddyCfg, b: &Buddy, busy: bool) -> Vec<Line<'static>> {
    let dim = |s: String| Span::styled(s, crate::theme::DIM());
    let pose = Pose { eyes: if b.mood(busy) == Mood::Sleeping { Eyes::Closed } else { Eyes::Happy }, dy: 1, ..b.pose(id, busy, 1000, true) };
    let prog = &b.progress;
    let level = prog.level();
    let (lo, hi) = (xp_for(level), xp_for(level + 1));
    let stat = |label: &str, v: u8| dim(format!("{label:<6} {} {v:>3}", bar(v as u64, 100, 10)));
    let mut info: Vec<Vec<Span<'static>>> = vec![
        vec![
            Span::styled(id.name.clone(), Style::new().fg(id.rarity.color()).add_modifier(Modifier::BOLD)),
            dim(format!("  {} {}", id.rarity.label(), id.species().name)),
        ],
        vec![Span::styled(id.species().blurb.to_string(), crate::theme::DIM().add_modifier(Modifier::ITALIC))],
        Vec::new(),
        vec![
            Span::styled(format!("Level {level}  "), Style::new().fg(id.rarity.color()).add_modifier(Modifier::BOLD)),
            Span::styled(bar(prog.xp - lo, hi - lo, 12), Style::new().fg(crate::skin::palette().accent)),
            dim(format!("  {}/{} xp", prog.xp - lo, hi - lo)),
        ],
        vec![stat("focus", id.stats[0])],
        vec![stat("chaos", id.stats[1])],
        vec![stat("snark", id.stats[2])],
        vec![dim(format!(
            "{} turns · {} fixes · {} chats · streak {} day{} (best {})",
            prog.turns,
            prog.fixes,
            prog.chats,
            prog.streak,
            if prog.streak == 1 { "" } else { "s" },
            prog.best_streak
        ))],
        vec![dim(format!("feeling {}", b.mood(busy).label()))],
    ];
    if let Some((at, what)) = prog.next_unlock() {
        info.push(vec![dim(format!("next: {what} at level {at}"))]);
    }
    let mut out = Vec::new();
    match crate::image::block(id, cfg, pose, 8, info.len()) {
        Some(img) => {
            // a real image on the left, the text beside it
            for (i, row) in info.into_iter().enumerate() {
                let mut spans = vec![Span::raw(" ".repeat(img.cols + 3))];
                spans.extend(row);
                let mut line = Line::from(spans);
                if i + 1 == img.rows {
                    line = img.mark(line);
                }
                out.push(line);
            }
            for i in out.len()..img.rows {
                let line = Line::default();
                out.push(if i + 1 == img.rows { img.mark(line) } else { line });
            }
        }
        None => {
            let art = pixel::half_blocks(&pixel::canvas(id.species(), &palette_for(id, cfg), pose), 2);
            let w = pixel::W * 2;
            for i in 0..art.len().max(info.len()) {
                let mut spans = vec![Span::raw(" ")];
                match art.get(i) {
                    Some(r) => spans.extend(r.iter().cloned()),
                    None => spans.push(Span::raw(" ".repeat(w))),
                }
                spans.push(Span::raw("   "));
                if let Some(r) = info.get(i) {
                    spans.extend(r.iter().cloned());
                }
                out.push(Line::from(spans));
            }
        }
    }
    out.push(Line::from(dim(" /pettalk <anything> · /buddy pet · trick · name · species · reroll · mini · quiet · off".into())));
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
        assert_eq!(a, hatch("ravth", &cfg), "same name, same pet");
        let mut other = cfg.clone();
        other.seed = 1;
        assert!(a != hatch("ravth", &other), "a reroll changes something");
        other.species = "Fox".into();
        other.name = "Pip".into();
        let c = hatch("ravth", &other);
        assert_eq!((c.species().name, c.name.as_str()), ("fox", "Pip"));
        assert!(c.stats.iter().all(|s| *s <= 100));
        other.species = "astrocat".into();
        assert_eq!(hatch("ravth", &other).species().name, "cat", "old names still hatch");
        assert!(!c.personality().is_empty());
    }

    #[test]
    fn sprites_are_sixteen_wide_and_five_tall() {
        let cfg = BuddyCfg::default();
        for i in 0..SPECIES.len() {
            let id = Identity { species: i, name: "x".into(), rarity: Rarity::Common, stats: [1, 2, 3] };
            let b = Buddy::new();
            let rows = sprite(&id, &cfg, b.pose(&id, false, 1000, true));
            assert_eq!(rows.len(), 5);
            for r in rows {
                assert_eq!(r.iter().map(|s| crate::wrap::width_of(&s.content)).sum::<usize>(), 16);
            }
        }
    }

    #[test]
    fn colours_can_be_overridden() {
        let mut cfg = BuddyCfg::default();
        cfg.colors.insert("body".into(), "#123456".into());
        let id = Identity { species: 0, name: "x".into(), rarity: Rarity::Common, stats: [1, 2, 3] };
        assert_eq!(palette_for(&id, &cfg).b, (0x12, 0x34, 0x56));
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
        let mut b = Buddy::new();
        b.on(Moment::Done { secs: 3 }, &cfg, "Zo");
        assert!(b.quip.is_none());
        assert_eq!(b.mood(false), Mood::Happy);
        b.on(Moment::Done { secs: 90 }, &cfg, "Zo");
        assert!(b.quip.is_some());
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
    fn happy_pets_hop_and_tricks_play() {
        let id = Identity { species: 0, name: "x".into(), rarity: Rarity::Common, stats: [1, 2, 3] };
        let mut b = Buddy::new();
        b.on(Moment::Pet, &BuddyCfg::default(), "x");
        let dys: Vec<usize> = (0..10).map(|i| b.pose(&id, false, i * 110, false).dy).collect();
        assert!(dys.contains(&0) && dys.contains(&1), "it hops: {dys:?}");
        assert!(b.pose(&id, false, 0, true).dy == 1, "reduced motion stays put");
        let mut b = Buddy::new();
        b.trick(Trick::Nap);
        assert_eq!(b.pose(&id, false, 500, false).eyes, Eyes::Closed);
        assert_eq!(Trick::parse("Jump"), Some(Trick::Hop));
    }

    #[test]
    fn progress_levels_and_streaks() {
        let mut p = Progress::default();
        assert_eq!(p.level(), 1);
        assert_eq!(p.gain_on(3, 100), None);
        assert_eq!(p.streak, 1);
        assert!(p.gain_on(20, 101).is_some(), "a level-up");
        assert_eq!(p.streak, 2);
        p.gain_on(1, 105);
        assert_eq!((p.streak, p.best_streak), (1, 2), "a gap resets the streak");
        assert_eq!(xp_for(1), 0);
        assert!(xp_for(10) > xp_for(5));
        assert!(p.next_unlock().is_some());
    }

    #[test]
    fn break_nudges_once_per_stretch() {
        let mut b = Buddy::new();
        assert!(!b.break_due(50));
        b.working_since = Some(Instant::now() - Duration::from_secs(51 * 60));
        assert!(b.break_due(50));
        assert!(!b.break_due(50), "not again right away");
        assert!(!b.break_due(0), "0 turns it off");
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
