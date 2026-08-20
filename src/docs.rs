//! The bundled documentation, compiled into the binary. Read it with
//! `gaff docs [topic]`.
//!
//! diataxis holds the listing, the lookup and the search. This module
//! holds the set, because a guard test reads the configuration page to
//! check that every shipped guard is documented.
//!
//! gaff takes the store half of diataxis and none of the clap surface.
//! clap exits 2 on a usage error, and 2 is the code gaff reserves for a
//! guard refusing a tool call.

use std::sync::OnceLock;

/// Every page in `docs/`, embedded by the build script. Nothing lists
/// them, so a page added to the directory is in the next build.
static PAGES: &[(&str, &str)] = diataxis::embedded_docs!();

/// The set, parsed once.
///
/// A page that does not parse is a build-time mistake in this
/// repository, not a runtime condition, so this panics rather than
/// carrying a Result through every caller.
pub fn set() -> &'static diataxis::DocSet {
    static SET: OnceLock<diataxis::DocSet> = OnceLock::new();
    SET.get_or_init(|| diataxis::DocSet::from_embedded(PAGES).expect("every bundled page parses"))
}

/// One page body, by slug, title, or unique prefix.
#[must_use]
pub fn topic(name: &str) -> Option<&'static str> {
    set().find(name).map(|page| page.content.as_str())
}

/// The topics a reader can ask for.
#[must_use]
pub fn listing() -> String {
    let mut out = String::from("Topics (gaff docs <topic>):\n");
    out.push_str(&set().listing());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_resolves_and_is_nonempty() {
        for page in set().pages() {
            assert!(
                topic(&page.slug).is_some_and(|b| !b.is_empty()),
                "{}",
                page.slug
            );
        }
        assert!(topic("nope").is_none());
    }

    #[test]
    fn every_page_declares_a_diataxis_type() {
        for page in set().pages() {
            assert!(page.kind.is_some(), "{} declares no type", page.slug);
        }
    }
}
