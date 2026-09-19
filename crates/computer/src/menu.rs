/// Result of trying to dispatch a menu path through the platform's native
/// accessibility API.
#[derive(Debug, PartialEq, Eq)]
pub enum NativeMenuOutcome {
    Pressed,
    NotFound {
        segment: String,
        available: Vec<String>,
    },
    Unavailable(String),
}

pub(crate) fn normalized_menu_segment(segment: &str) -> String {
    let mut segment = segment.trim();
    segment = segment.strip_suffix('…').unwrap_or(segment).trim_end();
    segment = segment.strip_suffix("...").unwrap_or(segment).trim_end();
    segment.to_lowercase()
}
