//! ztui — interactive terminal front-end for cosmostui.
//!
//! Two faces: an inline TUI in the Devin CLI style (history committed to
//! native scrollback, a live composer region redrawn beneath it) driving the
//! agent on a worker thread; and a line-oriented REPL fallback used when
//! there's no terminal (piped stdin/stdout, tests).

mod highlight;
#[cfg(feature = "highlight")]
mod highlight_ts;
mod line;
mod logo;
mod markdown;
mod motion;
mod skin;
mod slash;
mod term;
mod theme;
mod tui;
mod view;
mod files;
mod voice;
mod worker;
mod wrap;

use std::path::Path;

use anyhow::Result;
use zproto::RunOpts;

/// Interactive session loop. Returns the process exit code.
///
/// Uses the inline TUI when stdin+stdout are terminals; falls back to the
/// line REPL otherwise (or if terminal init fails).
pub fn repl(opts: &RunOpts, cwd: &Path) -> Result<i32> {
    match tui::try_run(opts, cwd) {
        Ok(Some(code)) => Ok(code),
        Ok(None) => line::repl(opts, cwd),
        Err(e) => Err(e),
    }
}

/// Building blocks `cosmostui-welcome` (the one-shot onboarding binary) paints
/// with — the same renderer, palette and matcher as the TUI.
#[doc(hidden)]
pub mod kit {
    pub use crate::slash::rank;
    pub use crate::term::write_line;
    pub use crate::theme::{bold, fg, ACCENT, ACCENT_BOLD, DIM, GHOST, GREEN, RED, RULE};
    pub use crate::wrap::{ellipsize, width_of};
}

/// Tests that set process-wide env vars (COSMOS_CLI_HOME, …) take this lock
/// so they don't race each other under the parallel test runner.
#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}
