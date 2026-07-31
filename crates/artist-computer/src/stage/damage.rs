//! Deciding when a screen has stopped changing.
//!
//! "No damage for N milliseconds" is the obvious predicate and it does not work.
//! A focused text field blinks its caret forever. A spinner spins. A clock ticks
//! once a second, and a video never stops at all. Waiting for silence on a real
//! screen means waiting for the timeout, every single time.
//!
//! So this classifies repeating small rectangles as **noise** and settles on the
//! absence of everything else. A caret is a handful of pixels redrawn on a
//! metronome; a dialog opening is not. The distinction is cheap to compute and
//! it is what makes `settle: quiet` a predicate rather than a synonym for the
//! timeout.
//!
//! Deliberately independent of the compositor: it consumes a stream of
//! rectangles and timestamps, so its behaviour is pinned by unit tests that feed
//! synthetic damage rather than by trying to provoke a real application into
//! blinking.

use std::collections::HashMap;
use std::time::Duration;

use crate::model::Rect;

/// A rect must repeat at least this often to be treated as noise.
const NOISE_REPEATS: u32 = 3;
/// …within this window.
const NOISE_WINDOW: Duration = Duration::from_secs(2);
/// …and cover no more than this fraction of the screen. A repeating region
/// larger than this is something the model should hear about even if it
/// repeats — a video playing is a fact about the page.
const NOISE_AREA_FRACTION: f64 = 0.01;

/// Tracks which regions of a screen are merely animating.
#[derive(Debug)]
pub struct NoiseFilter {
    screen_area: f64,
    /// Rect signature -> (times seen, when *last* seen).
    ///
    /// Last, not first: the window is a gap detector. Keying it to the first
    /// sighting would declassify a caret that has been blinking steadily for
    /// longer than the window, which is precisely the case it exists to catch.
    seen: HashMap<(i32, i32, u32, u32), (u32, Duration)>,
}

impl NoiseFilter {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            screen_area: f64::from(screen_width) * f64::from(screen_height),
            seen: HashMap::new(),
        }
    }

    /// Record a damage rectangle and report whether it is meaningful.
    ///
    /// `at` is elapsed time since the filter started, passed in rather than read
    /// from a clock so the behaviour is testable without sleeping.
    pub fn observe(&mut self, rect: Rect, at: Duration) -> bool {
        let area = f64::from(rect.width) * f64::from(rect.height);
        let small = self.screen_area > 0.0 && area / self.screen_area <= NOISE_AREA_FRACTION;
        if !small {
            return true;
        }

        let key = (rect.x, rect.y, rect.width, rect.height);
        let entry = self.seen.entry(key).or_insert((0, at));
        // Restart the count if the gap since the last sighting exceeds the
        // window: a rect that blinked a minute ago and is only now changing
        // again is news, but one that has never stopped is still a metronome.
        if at.saturating_sub(entry.1) > NOISE_WINDOW {
            entry.0 = 0;
        }
        entry.0 += 1;
        entry.1 = at;
        entry.0 < NOISE_REPEATS
    }

    /// Regions currently classified as noise, for diagnostics.
    pub fn noise_rects(&self) -> usize {
        self.seen
            .values()
            .filter(|(count, _)| *count >= NOISE_REPEATS)
            .count()
    }
}

/// Accumulates damage and answers "has it gone quiet?".
#[derive(Debug)]
pub struct QuietTracker {
    filter: NoiseFilter,
    /// When meaningful damage was last seen.
    last_meaningful: Option<Duration>,
    /// Whether anything meaningful has been seen at all.
    saw_any: bool,
    quiet_for: Duration,
}

impl QuietTracker {
    pub fn new(screen_width: u32, screen_height: u32, quiet_for: Duration) -> Self {
        Self {
            filter: NoiseFilter::new(screen_width, screen_height),
            last_meaningful: None,
            saw_any: false,
            quiet_for,
        }
    }

    pub fn observe(&mut self, rect: Rect, at: Duration) {
        if self.filter.observe(rect, at) {
            self.saw_any = true;
            self.last_meaningful = Some(at);
        }
    }

    /// Settled means something happened and then stopped happening.
    ///
    /// Requiring that something happened first is what stops a program from
    /// "settling" instantly on a screen that never reacted to it at all — which
    /// would report success for an action that did nothing.
    pub fn settled(&self, now: Duration) -> bool {
        match (self.saw_any, self.last_meaningful) {
            (true, Some(last)) => now.saturating_sub(last) >= self.quiet_for,
            _ => false,
        }
    }

    pub fn noise_rects(&self) -> usize {
        self.filter.noise_rects()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 1920;
    const H: u32 = 1080;

    fn caret() -> Rect {
        Rect {
            x: 100,
            y: 200,
            width: 2,
            height: 16,
        }
    }

    fn dialog() -> Rect {
        Rect {
            x: 400,
            y: 300,
            width: 600,
            height: 400,
        }
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn a_blinking_caret_becomes_noise_after_a_few_repeats() {
        let mut filter = NoiseFilter::new(W, H);
        // The first sightings are indistinguishable from a real change.
        assert!(filter.observe(caret(), ms(0)));
        assert!(filter.observe(caret(), ms(500)));
        // By the third it is clearly a metronome.
        assert!(!filter.observe(caret(), ms(1000)));
        assert!(!filter.observe(caret(), ms(1500)));
        assert_eq!(filter.noise_rects(), 1);
    }

    #[test]
    fn a_large_region_is_never_noise_however_often_it_repeats() {
        let mut filter = NoiseFilter::new(W, H);
        for tick in 0..20 {
            assert!(
                filter.observe(dialog(), ms(tick * 100)),
                "a large repeating region is a fact about the screen, not noise"
            );
        }
        assert_eq!(filter.noise_rects(), 0);
    }

    #[test]
    fn a_caret_stays_noise_however_long_it_keeps_blinking() {
        // The window is a gap detector, not a lifetime. A caret blinking
        // steadily for far longer than NOISE_WINDOW must stay classified —
        // otherwise `quiet` starts timing out again on any focused text field.
        let mut filter = NoiseFilter::new(W, H);
        for tick in 0..200u64 {
            filter.observe(caret(), ms(tick * 500));
        }
        assert_eq!(filter.noise_rects(), 1);
        assert!(!filter.observe(caret(), ms(100_000)));
    }

    #[test]
    fn a_rect_that_stops_and_later_resumes_is_news_again() {
        let mut filter = NoiseFilter::new(W, H);
        for tick in 0..4 {
            filter.observe(caret(), ms(tick * 200));
        }
        // Long silence, then it moves again — that is a change, not the same
        // metronome still ticking.
        assert!(filter.observe(caret(), ms(10_000)));
    }

    #[test]
    fn quiet_settles_once_real_damage_stops_even_while_a_caret_blinks() {
        let mut tracker = QuietTracker::new(W, H, ms(250));
        tracker.observe(dialog(), ms(0));
        assert!(!tracker.settled(ms(100)), "still within the quiet window");

        // Establish the caret as noise, then keep it blinking forever.
        for tick in 0..3 {
            tracker.observe(caret(), ms(200 + tick * 50));
        }
        for tick in 0..20 {
            tracker.observe(caret(), ms(400 + tick * 100));
        }

        assert!(
            tracker.settled(ms(3_000)),
            "a blinking caret must not hold the screen open forever"
        );
        assert_eq!(tracker.noise_rects(), 1);
    }

    #[test]
    fn a_screen_that_never_reacted_does_not_report_as_settled() {
        let tracker = QuietTracker::new(W, H, ms(250));
        assert!(
            !tracker.settled(ms(10_000)),
            "settling on silence would report success for an action that did nothing"
        );
    }

    #[test]
    fn continuing_real_damage_holds_the_screen_open() {
        let mut tracker = QuietTracker::new(W, H, ms(250));
        for tick in 0..20 {
            tracker.observe(dialog(), ms(tick * 100));
            assert!(
                !tracker.settled(ms(tick * 100 + 50)),
                "a screen still changing must never be called settled"
            );
        }
    }
}
