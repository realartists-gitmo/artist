//! The name roster, compiled in from `docs/artists.md`.
//!
//! Embedded at build time rather than read at run time for two reasons: a
//! broken or partial install cannot leave agents unnameable, and every machine
//! that might exchange messages agrees on the set without shipping a data file
//! alongside the binary.
//!
//! `docs/artists.md` stays the single source of truth — there is no generated
//! copy to drift from it. The cost is that this crate knows its position in the
//! workspace, which is acceptable for a workspace-internal crate and would not
//! be if it were published.

use std::sync::LazyLock;

static SOURCE: &str = include_str!("../../../docs/artists.md");

static NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    SOURCE
        .lines()
        .map(str::trim)
        // Tolerate blank lines and any prose or markdown heading the file
        // picks up later, so editing the roster never means editing a parser.
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
});

/// The roster as a slice. Named in caps because it reads as a constant at every
/// use site, which is what it is once initialised.
#[allow(non_upper_case_globals)]
pub static ROSTER: RosterHandle = RosterHandle;

/// Deref shim so `ROSTER` can be used as a slice without callers thinking about
/// the lazy initialisation behind it.
#[derive(Clone, Copy, Debug)]
pub struct RosterHandle;

impl std::ops::Deref for RosterHandle {
    type Target = [&'static str];

    fn deref(&self) -> &Self::Target {
        NAMES.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roster_parses_to_plain_names() {
        assert_eq!(ROSTER.len(), 736, "the identity roster is contractual");
        assert!(ROSTER.contains(&"Monet"));
        assert!(
            ROSTER.iter().all(|name| !name.is_empty()),
            "blank lines must not become names"
        );
        assert!(
            ROSTER.iter().all(|name| !name.starts_with('#')),
            "a heading must not become a name"
        );
    }
}
