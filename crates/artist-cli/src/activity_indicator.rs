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

pub(crate) fn activity_status(phase: &str, elapsed: std::time::Duration, frame: usize) -> String {
    format!(
        "{} {phase} [{} elapsed]",
        activation_indicator(frame),
        format_elapsed(elapsed)
    )
}

pub(crate) fn format_elapsed(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
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

    #[test]
    fn status_combines_phase_animation_and_elapsed_time() {
        assert_eq!(
            activity_status("working", std::time::Duration::from_secs(65), 3),
            "⋮··⋮ working [01:05 elapsed]"
        );
        assert_eq!(
            format_elapsed(std::time::Duration::from_secs(3_661)),
            "01:01:01"
        );
    }
}
