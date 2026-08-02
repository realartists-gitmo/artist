//! Rung 3 as a surface you can act on.
//!
//! The last rung, and until now the one that gave up. A window with no
//! accessibility tree, no debugging protocol and no adapter used to report "no
//! actionable surface" — honest, but a dead end for games, canvas applications
//! and anything whose accessibility support was never finished.
//!
//! What changed is *who* names the position. The original reasoning was that
//! minting anchors from pixels is coordinates wearing a hat; the mistake was
//! concluding that pixels are therefore unusable. Coordinates are only dangerous
//! when the **model** supplies them. Here the harness reads the screen, finds
//! the text, and mints anchors for it — and the model says `click kv7` exactly
//! as it does at every other rung. The contract is untouched: the model names
//! things, and a name that no longer resolves is an error rather than a click on
//! whatever moved into that spot.
//!
//! Three things make this cheap for us specifically, and all come from owning
//! the compositor. Capture is a buffer read rather than a screencast
//! negotiation. **Damage tells us what to re-read** — measured at 15× on a
//! 1920×1080 frame, worth more than every other optimization on this rung
//! combined. And a capture contains *only this window*, because the compositor
//! recomposites for it: every toplevel here is given the whole output, so a
//! surface that merely cropped the screen would read every other application's
//! text as though it were its own.

use std::sync::{Arc, Mutex};

use crate::model::{Caps, Frame, Node, Role, Rung, Snapshot, SurfaceId};
use crate::ocr::{DetectOptions, Incremental, Ocr};
use crate::program::{Settle, SettleKind, SettleOutcome, Step, StepError};
use crate::stage::{Stage, WindowKey};
use crate::surface::{SettleWatch, Surface};

/// How long to let damage accumulate before deciding a screen has settled.
const QUIET_MS: u64 = 250;
/// Poll granularity while waiting.
const POLL_MS: u64 = 30;

/// A window driven entirely from what is drawn on it.
pub struct ScreenSurface {
    id: SurfaceId,
    stage: Arc<dyn Stage>,
    window: WindowKey,
    ocr: Ocr,
    options: DetectOptions,
    /// Text found so far, and the machinery that re-reads only what changed.
    state: Mutex<Incremental>,
    /// Damage seen since the last observation, in frame coordinates.
    pending: Arc<Mutex<Vec<crate::model::Rect>>>,
}

impl ScreenSurface {
    pub fn new(id: impl Into<String>, stage: Arc<dyn Stage>, window: WindowKey, ocr: Ocr) -> Self {
        let pending: Arc<Mutex<Vec<crate::model::Rect>>> = Arc::default();

        // Damage arrives continuously and is consumed at observation time.
        // Collecting it in the background is what makes the incremental read
        // possible: by the time anyone asks, we already know what moved.
        let sink = Arc::clone(&pending);
        let mut receiver = stage.damage();
        let watched = window;
        tokio::spawn(async move {
            while let Ok(damage) = receiver.recv().await {
                if damage.window.is_some_and(|key| key != watched) {
                    continue;
                }
                let mut held = sink.lock().unwrap();
                // Bounded. A window redrawing continuously would otherwise grow
                // this without limit between observations, and past a few dozen
                // rectangles the coalescer is going to merge them anyway.
                if held.len() < 256 {
                    held.push(damage.region);
                }
            }
        });

        Self {
            id: SurfaceId::new(id),
            stage,
            window,
            ocr,
            options: DetectOptions::default(),
            state: Mutex::new(Incremental::new()),
            pending,
        }
    }

    /// Discard what we believe is on screen, so the next look reads everything.
    pub fn invalidate(&self) {
        self.state.lock().unwrap().reset();
    }

    fn take_damage(&self) -> Vec<crate::model::Rect> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }

    async fn capture(&self) -> Result<Frame, StepError> {
        self.stage.capture(Some(self.window)).await
    }

    /// Read the screen, re-reading only what the compositor says changed.
    async fn refresh(&self) -> Result<Vec<Node>, StepError> {
        let frame = self.capture().await?;
        let damage = self.take_damage();

        let mut state = self.state.lock().unwrap();
        let boxes = state
            .update(&self.ocr, &frame, &damage, &self.options)
            .map_err(StepError::Backend)?;

        Ok(boxes
            .iter()
            .enumerate()
            .map(|(index, found)| {
                // The binding is the position, quantized. It has to be stable
                // across looks or every observation would churn every anchor —
                // and unlike a tree, pixels give us no identity to borrow. Text
                // that moves genuinely *is* a different element as far as this
                // rung can tell, and saying so is more honest than pretending an
                // identity we cannot verify.
                let binding = format!("screen:{}:{}:{}", found.rect.x / 4, found.rect.y / 4, index);
                Node::new(binding, Role::Text, found.text.clone())
                    .with_bounds(found.rect)
                    .with_actions(["click"])
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl Surface for ScreenSurface {
    fn id(&self) -> &SurfaceId {
        &self.id
    }

    fn rung(&self) -> Rung {
        Rung::Pixels
    }

    /// Honest about the one thing this rung cannot do.
    ///
    /// Clicking works because we find the target ourselves. Typing and keys go
    /// to whatever holds focus, which the stage handles. Scrolling has no
    /// meaning without a scrollable container we can identify, and we cannot.
    fn caps(&self) -> Caps {
        Caps {
            click: true,
            type_text: false,
            key: true,
            scroll: false,
            pixels: true,
        }
    }

    fn title(&self) -> String {
        self.id.as_str().to_owned()
    }

    async fn snapshot(&self) -> Result<Snapshot, StepError> {
        Ok(Snapshot::new(self.refresh().await?))
    }

    async fn snapshot_full(&self) -> Result<Snapshot, StepError> {
        self.invalidate();
        Ok(Snapshot::new(self.refresh().await?))
    }

    async fn pixels(&self) -> Result<Option<Frame>, StepError> {
        self.capture().await.map(Some)
    }

    /// Settle on damage going quiet.
    ///
    /// The best change signal on this rung by some distance, and it comes free
    /// with owning the compositor: there is no tree to diff and no readiness
    /// flag to poll, but we are *told* when pixels stop moving.
    async fn watch(&self, settle: &Settle) -> Result<SettleWatch, StepError> {
        if matches!(settle.until, SettleKind::None) {
            return Ok(SettleWatch::ready(SettleOutcome::Settled { after_ms: 0 }));
        }
        if matches!(settle.until, SettleKind::NetworkIdle) {
            return Ok(SettleWatch::ready(SettleOutcome::Unsupported));
        }

        let pending = Arc::clone(&self.pending);
        let timeout = settle.timeout_ms;
        let started = std::time::Instant::now();
        let armed = pending.lock().unwrap().len();

        Ok(SettleWatch(Box::new(Box::pin(async move {
            let deadline = started + std::time::Duration::from_millis(timeout);
            let mut last_count = armed;
            let mut quiet_since: Option<std::time::Instant> = None;
            let mut moved = false;

            loop {
                tokio::time::sleep(std::time::Duration::from_millis(POLL_MS)).await;
                let count = pending.lock().unwrap().len();

                if count != last_count {
                    moved = true;
                    last_count = count;
                    quiet_since = None;
                } else {
                    let since = *quiet_since.get_or_insert_with(std::time::Instant::now);
                    // Something happened, and then stopped happening. Requiring
                    // the change first is what stops an idle screen reporting
                    // "settled" before the click has had any effect.
                    if moved && since.elapsed() >= std::time::Duration::from_millis(QUIET_MS) {
                        return SettleOutcome::Settled {
                            after_ms: started.elapsed().as_millis() as u64,
                        };
                    }
                }

                if std::time::Instant::now() >= deadline {
                    return SettleOutcome::TimedOut {
                        after_ms: started.elapsed().as_millis() as u64,
                    };
                }
            }
        }))))
    }

    async fn apply(&self, step: &Step, node: Option<&Node>) -> Result<(), StepError> {
        match step {
            Step::Click(target) => {
                let node = node.ok_or_else(|| {
                    StepError::Backend("a click on a screen surface needs an element".into())
                })?;
                let bounds = node.bounds.ok_or_else(|| {
                    StepError::Backend(format!(
                        "{} has no position on screen — re-observe {}",
                        target.anchor, self.id
                    ))
                })?;
                // The centre, computed here from what we detected. The model
                // never supplied a coordinate and never sees one.
                self.stage.pointer(self.window, bounds, 0x110).await?;
                // What we clicked has probably changed, and the rest of the
                // screen has not — damage will say which parts.
                Ok(())
            }
            Step::Key(press) => self.stage.key(self.window, press.chord()).await,
            other => Err(StepError::Backend(format!(
                "a screen surface cannot {:?} — it can click what it can read, and send keys. \
                 If this application has a debugging protocol or an accessibility tree, it \
                 should not be on this rung at all.",
                other.action()
            ))),
        }
    }
}
