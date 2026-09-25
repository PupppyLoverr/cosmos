//! o200k_base token counting, fully offline.
//!
//! The BPE rank table is vendored at `vendor/o200k_base.tiktoken` (downloaded
//! once at build time from openaipublic.blob.core.windows.net — the canonical
//! public mirror). Nothing is fetched at run time. We reimplement the standard
//! byte-pair-merge over the vendored ranks and the published o200k split
//! pattern. o200k_base is the encoding used by current OpenAI and
//! OpenAI-compatible models and is our canonical counting unit across harnesses.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use base64::Engine;
use fancy_regex::Regex;

/// The o200k_base split pattern (published in the tiktoken spec).
const O200K_PATTERN: &str = concat!(
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"|[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"|\p{N}{1,3}",
    r"| ?[^\s\p{L}\p{N}]+[\r\n/]*",
    r"|\s*[\r\n]+",
    r"|\s+(?!\S)",
    r"|\s+"
);

struct Encoder {
    ranks: HashMap<Vec<u8>, u32>,
    pattern: Regex,
}

static ENCODER: OnceLock<Encoder> = OnceLock::new();

fn load() -> Result<Encoder> {
    let data = include_bytes!("../vendor/o200k_base.tiktoken");
    let mut ranks = HashMap::with_capacity(200_000);
    for line in data.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let sp = line.iter().position(|&b| b == b' ').context("bad rank line")?;
        let tok = base64::engine::general_purpose::STANDARD.decode(&line[..sp])?;
        let rank: u32 = std::str::from_utf8(&line[sp + 1..])?.trim().parse()?;
        ranks.insert(tok, rank);
    }
    let pattern = Regex::new(O200K_PATTERN).context("compiling o200k pattern")?;
    Ok(Encoder { ranks, pattern })
}

fn encoder() -> &'static Encoder {
    ENCODER.get_or_init(|| load().expect("o200k ranks must load"))
}

/// Greedy lowest-rank merge: repeatedly fuse the adjacent pair whose span has
/// the lowest rank until no pair is a known token.
fn byte_pair_encode(piece: &[u8], ranks: &HashMap<Vec<u8>, u32>) -> Vec<u32> {
    if piece.len() == 1 {
        return vec![*ranks.get(piece).unwrap_or(&u32::MAX)];
    }
    let mut bounds: Vec<usize> = (0..=piece.len()).collect();
    loop {
        let mut best: Option<(u32, usize)> = None;
        for i in 0..bounds.len().saturating_sub(2) {
            let span = &piece[bounds[i]..bounds[i + 2]];
            if let Some(&r) = ranks.get(span) {
                if best.is_none_or(|(br, _)| r < br) {
                    best = Some((r, i));
                }
            }
        }
        match best {
            Some((_, i)) => {
                bounds.remove(i + 1);
            }
            None => break,
        }
        if bounds.len() <= 2 {
            break;
        }
    }
    bounds
        .windows(2)
        .map(|w| *ranks.get(&piece[w[0]..w[1]]).unwrap_or(&u32::MAX))
        .collect()
}

/// Number of o200k_base tokens in `text`.
pub fn count(text: &str) -> usize {
    let enc = encoder();
    let mut n = 0usize;
    for m in enc.pattern.find_iter(text).flatten() {
        let piece = m.as_str().as_bytes();
        if piece.is_empty() {
            continue;
        }
        if enc.ranks.contains_key(piece) {
            n += 1;
        } else {
            n += byte_pair_encode(piece, &enc.ranks).len();
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_vendored_ranks() {
        let enc = encoder();
        assert!(enc.ranks.len() > 190_000, "expected ~200k ranks");
    }

    #[test]
    fn counts_english() {
        // Reference values checked against tiktoken o200k_base.
        assert_eq!(count("hello world"), 2);
        assert_eq!(count("The quick brown fox jumps over the lazy dog"), 9);
        assert_eq!(count(""), 0);
        assert_eq!(count("a"), 1);
    }

    #[test]
    fn counts_code_and_unicode() {
        let n = count("fn main() { println!(\"hi\"); }");
        assert!(n > 5 && n < 30);
        // multi-byte unicode should still encode
        let n2 = count("héllo wörld — ünïcodé");
        assert!(n2 > 2);
    }
}
