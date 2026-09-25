//! Secret scanner: rejects memory text that looks like a credential.

/// Patterns that always reject, with the reason shown to the caller.
const DENY_SUBSTRINGS: &[(&str, &str)] = &[
    ("sk-", "contains sk- token marker"),
    ("xai-", "contains xai- token marker"),
    ("AKIA", "contains AWS access-key marker"),
    ("ghp_", "contains GitHub PAT marker"),
    ("-----BEGIN", "contains PEM block"),
    ("Bearer ", "contains Bearer credential"),
];

/// Shannon entropy (bits per char) of `s`.
fn entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

fn is_base64ish(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'-' | b'_')
}

/// Returns `Err(reason)` when `text` should not be stored.
pub fn scan(text: &str) -> Result<(), String> {
    for (needle, reason) in DENY_SUBSTRINGS {
        if text.contains(needle) {
            return Err((*reason).to_string());
        }
    }
    // Any single token of >=32 chars with base64/hex alphabet and entropy > 4.0.
    for word in text.split(|c: char| c.is_whitespace() || c == '"') {
        let word = word.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || "+/=_-".contains(c)));
        if word.len() >= 32
            && word.bytes().all(is_base64ish)
            && entropy(word) > 4.0
        {
            return Err(format!(
                "contains a {}-char high-entropy token (entropy {:.2})",
                word.len(),
                entropy(word)
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_known_markers() {
        for bad in [
            "key is sk-abc123",
            "xai-ABCDEFGH",
            "aws AKIAIOSFODNN7EXAMPLE",
            "ghp_abcdefghijklmnop",
            "-----BEGIN PRIVATE KEY-----",
            "curl -H 'Bearer abcdef'",
            // 48-char high-entropy base64ish token
            "token Zj3kQ9mXv2Lp8Rt5Yw7Nb4Cd6Fg1Hj0Ks3Mn5Bv8Cx2Zq4We",
        ] {
            assert!(scan(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn accepts_normal_text() {
        assert!(scan("prefer squash merges for release branches").is_ok());
        assert!(scan("the release ran on 2026-09-22").is_ok());
        // Hex-looking but low-entropy (repeated chars) is fine.
        assert!(scan("commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_ok());
    }
}
