//! `BDO_QUIET=1`: silence bdo's own advisory notes on stderr.
//!
//! Only notes that point somewhere else are suppressed — the reduced-view hint
//! after `bdo read`, the daily "no hook installed / hook outdated" nag, and the
//! "--raw has no effect" note. Errors, warnings that bdo itself misbehaved (a
//! filter panicked, the hook failed its integrity check), the wrapped
//! command's own stderr, and the `[full output: …]` tee pointer (part of the
//! output an agent recovers from) are never affected.

/// True when `BDO_QUIET` is set to a truthy value (1/true/yes/on).
pub fn is_quiet() -> bool {
    std::env::var("BDO_QUIET")
        .map(|v| super::raw::is_truthy(&v))
        .unwrap_or(false)
}

/// Print an advisory note to stderr unless `BDO_QUIET` is set.
pub fn note(msg: impl std::fmt::Display) {
    if !is_quiet() {
        eprintln!("{msg}");
    }
}
