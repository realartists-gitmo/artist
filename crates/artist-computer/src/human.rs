//! Humanish pacing for input delivery.
//!
//! The harness can synthesize input at any speed, and that is the problem:
//! perfectly uniform timing is the signature of a machine. A person keys in a
//! password at 80 ms per keystroke plus or minus, pauses on a hard letter, and
//! lingers half a beat before releasing a button. None of that is detectable
//! by counting — it is detectable by *not varying*.
//!
//! This module is the one place such delays are decided, so every surface
//! (CDP, the seat, the PTY) draws from the same well. The distribution is a
//! flat spread around a mean, which is enough: the point is variability with a
//! bounded, session-shaped range, not statistical authenticity. Never a
//! deterministic delay, and never a perfect cadence.
//!
//! All delays here live on the caller's task, never on the compositor thread.

use std::sync::atomic::{AtomicU64, Ordering};

/// A thread-safe, dependency-free pseudo-random source.
///
/// SplitMix64: enough state to be uncorrelated from call to call, no external
/// entropy dependency, and safe to seed cheaply from the clock. The seed is
/// not cryptographic and nothing here needs it to be — the requirement is that
/// two runs do not share a cadence, not that an observer cannot predict the
/// next delay.
struct Rng {
    state: u64,
}

impl Rng {
    fn seeded() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let tick = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            state: nanos ^ tick.rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// The next u64, mixed.
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A uniform value in `[0, limit)`.
    fn below(&mut self, limit: u64) -> u64 {
        // `next` is a full period mix, so masking to a power of two is uniform
        // enough for pacing; avoiding the modulo bias dance keeps the source
        // obviously correct.
        if limit == 0 {
            return 0;
        }
        self.next() % limit
    }
}

thread_local! {
    static RNG: std::cell::RefCell<Rng> = std::cell::RefCell::new(Rng::seeded());
}

/// A delay in `[base, base + spread]` milliseconds.
///
/// `base` is the mean and `spread` how much slower it may get. Zero spread is
/// a permitted call — a caller that wants a fixed floor — but the default
/// should always carry spread, because a constant delay is the machine tell
/// this module exists to remove.
pub fn ms(base: u64, spread: u64) -> std::time::Duration {
    RNG.with(|rng| {
        let mut rng = rng.borrow_mut();
        std::time::Duration::from_millis(base + rng.below(spread.saturating_add(1)))
    })
}

/// A jittered pause between two keystrokes.
pub fn between_keys() -> std::time::Duration {
    ms(55, 70)
}

/// A jittered pause between two mouse move samples of an approach path.
pub fn move_sample() -> std::time::Duration {
    ms(8, 10)
}

/// A jittered pause between pressing and releasing a button.
pub fn click_dwell() -> std::time::Duration {
    ms(55, 90)
}

/// A jittered pause between moving to an element and pressing on it.
pub fn pre_click_linger() -> std::time::Duration {
    ms(60, 120)
}

/// A jittered pause between two wheel ticks.
pub fn wheel_ticks() -> std::time::Duration {
    ms(24, 40)
}

/// A jittered pause between two `Scroll` steps in a session.
pub fn between_steps() -> std::time::Duration {
    ms(120, 220)
}
