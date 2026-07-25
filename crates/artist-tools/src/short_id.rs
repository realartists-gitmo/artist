const ADJECTIVES: &[&str] = &[
    "amber", "brisk", "calm", "clear", "coral", "crisp", "dawn", "deep", "fair", "fast", "gentle",
    "gold", "green", "keen", "light", "lunar", "misty", "navy", "quiet", "rapid", "red", "silver",
    "solar", "soft", "still", "swift", "teal", "warm", "wild", "wise", "young", "zen",
];

const NOUNS: &[&str] = &[
    "badger", "brook", "cedar", "comet", "crane", "dove", "ember", "falcon", "fern", "fox",
    "grove", "harbor", "heron", "lake", "lark", "maple", "mesa", "moon", "oak", "orchid", "otter",
    "pine", "raven", "reef", "river", "robin", "sparrow", "star", "stone", "tiger", "willow",
    "wolf",
];

/// Generate a compact, human-readable process-local identifier.
pub fn short_id(prefix: &str) -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let adjective = ADJECTIVES[usize::from(bytes[0]) % ADJECTIVES.len()];
    let noun = NOUNS[usize::from(bytes[1]) % NOUNS.len()];
    format!("{prefix}-{adjective}-{noun}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_short_and_readable() {
        let id = short_id("a");
        assert!(id.starts_with("a-"));
        assert_eq!(id.matches('-').count(), 2);
        assert!(id.len() <= 20);
    }
}
