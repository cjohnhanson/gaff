//! The bundled documentation, compiled into the binary. Read it with
//! `gaff docs [topic]`.

pub const TOPICS: [(&str, &str); 3] = [
    (
        "getting-started",
        include_str!("../docs/getting-started.md"),
    ),
    ("configuration", include_str!("../docs/configuration.md")),
    ("how-it-works", include_str!("../docs/how-it-works.md")),
];

#[must_use]
pub fn topic(name: &str) -> Option<&'static str> {
    TOPICS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, body)| *body)
}

#[must_use]
pub fn listing() -> String {
    let mut out = String::from("Topics (gaff docs <topic>):\n");
    for (name, _) in TOPICS {
        out.push_str("  ");
        out.push_str(name);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_resolves_and_is_nonempty() {
        for (name, _) in TOPICS {
            assert!(topic(name).is_some_and(|b| !b.is_empty()), "{name}");
        }
        assert!(topic("nope").is_none());
    }
}
