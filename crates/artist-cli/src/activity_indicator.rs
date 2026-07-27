const FRAMES: [&str; 7] = [
    "⋮·⋮·⋮",
    "·⋮·⋮",
    "··⋮",
    "⋮··⋮",
    "⋮··",
    "⋮·⋮·",
    "⋮·⋮·⋮",
];

pub(crate) fn activation_indicator(frame: usize) -> &'static str {
    FRAMES[frame % FRAMES.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_propagate_across_the_weight_stacks_and_loop() {
        assert_eq!(
            (0..FRAMES.len())
                .map(activation_indicator)
                .collect::<Vec<_>>(),
            FRAMES
        );
        assert_eq!(activation_indicator(FRAMES.len()), FRAMES[0]);
        assert_eq!(activation_indicator(FRAMES.len() + 1), FRAMES[1]);
    }

    #[test]
    fn every_frame_has_the_same_compact_display_width() {
        use unicode_width::UnicodeWidthStr;

        assert!(FRAMES.iter().all(|frame| frame.width() == 5));
    }
}
