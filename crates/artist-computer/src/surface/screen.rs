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

use crate::keys::parse_modifiers;
use crate::model::{Caps, Frame, Node, Role, Rung, Snapshot, SurfaceId};
use crate::ocr::{DetectOptions, Incremental, Ocr};
use crate::program::{Settle, SettleKind, SettleOutcome, Step, StepError};
use crate::stage::{Stage, WindowKey};
use crate::surface::{SettleWatch, Surface};

/// How long to let damage accumulate before deciding a screen has settled.
const QUIET_MS: u64 = 250;
/// Poll granularity while waiting.
const POLL_MS: u64 = 30;
/// How long a long press holds.
///
/// Android's own threshold is 500 ms, so this clears it with margin. Sitting
/// exactly on the threshold makes the gesture a coin toss decided by scheduling.
const LONG_PRESS_MS: u64 = 600;
/// How long a swipe takes to travel.
///
/// Short enough to feel deliberate, long enough that the velocity tracker sees
/// several points and reads it as a gesture rather than a teleport.
const SWIPE_MS: u64 = 250;

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
    /// The last capture, kept only for surfaces whose damage is too coarse to
    /// use. It is `None` until the first coarse frame, so a well-behaved client
    /// never pays for the memory.
    previous: Mutex<Option<Frame>>,
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
            previous: Mutex::new(None),
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

    /// This window's geometry, for actions that need a point but were given no
    /// element.
    async fn window_bounds(&self) -> Result<crate::model::Rect, StepError> {
        self.stage
            .windows()
            .await?
            .into_iter()
            .find(|window| window.key == self.window)
            .map(|window| window.geometry)
            .ok_or_else(|| {
                StepError::Backend(format!(
                    "{} is no longer on the stage — its window has gone",
                    self.id
                ))
            })
    }

    /// Where a named element is on screen.
    ///
    /// Every pointer verb on this rung starts here, and every one of them fails
    /// the same two ways: the step named nothing, or what it named has no
    /// position because the screen has been redrawn since it was read.
    fn bounds_of(
        &self,
        node: Option<&Node>,
        target: &crate::program::Target,
    ) -> Result<crate::model::Rect, StepError> {
        let node = node.ok_or_else(|| {
            StepError::Backend("this step on a screen surface needs an element".into())
        })?;
        node.bounds.ok_or_else(|| {
            StepError::Backend(format!(
                "{} has no position on screen — re-observe {}",
                target.anchor, self.id
            ))
        })
    }

    /// Where a drag ends: a second named element, or a direction and a distance.
    ///
    /// Exactly one of the two. Checked here rather than at parse time so the
    /// error can name the surface the model was working on, and so a backend
    /// that supports only one of the forms can say which.
    async fn drag_destination(
        &self,
        from: crate::model::Rect,
        to: Option<&Node>,
        direction: Option<crate::program::Direction>,
        distance: Option<u32>,
    ) -> Result<crate::model::Rect, StepError> {
        match (to, direction) {
            (Some(_), Some(_)) => Err(StepError::Backend(
                "a drag takes either a destination element or a direction, not both".into(),
            )),
            (None, None) => Err(StepError::Backend(
                "a drag needs somewhere to go: name a destination element with `to`, or give \
                 a `direction`"
                    .into(),
            )),
            (Some(node), None) => node.bounds.ok_or_else(|| {
                StepError::Backend(format!(
                    "the drag destination has no position on screen — re-observe {}",
                    self.id
                ))
            }),
            (None, Some(direction)) => {
                let window = self.window_bounds().await?;
                let span = match direction {
                    crate::program::Direction::Up | crate::program::Direction::Down => {
                        window.height
                    }
                    _ => window.width,
                };
                // A quarter of the screen by default, where a swipe uses half:
                // a drag is a deliberate reposition and usually wants to land
                // somewhere specific, so overshooting is the worse error.
                let travel = distance.unwrap_or(span / 4).min(span) as i32;
                let (dx, dy) = direction.offset(travel);
                Ok(crate::model::Rect {
                    x: from.x + dx,
                    y: from.y + dy,
                    width: from.width,
                    height: from.height,
                })
            }
        }
    }

    /// Read the screen, re-reading only what changed.
    ///
    /// Compositor damage is believed whenever it is specific, because a client
    /// knew what it drew before the pixels existed. When it is not — Android
    /// reports one full-surface rectangle per frame and nothing finer — the
    /// locality is recovered by comparing this capture with the last one. That
    /// costs a memcmp over the frame; the alternative costs an OCR pass over all
    /// of it.
    async fn refresh(&self) -> Result<Vec<Node>, StepError> {
        let frame = self.capture().await?;
        let mut damage = self.take_damage();

        if crate::stage::diff::is_coarse(&damage, &frame) {
            let mut previous = self.previous.lock().unwrap();
            if let Some(before) = previous.as_ref() {
                let regions = crate::stage::diff::changed_regions(before, &frame);
                // An empty result is meaningful: the client said everything
                // changed and nothing actually did — an animation frame that
                // repainted identical pixels. Believing the pixels saves the
                // entire read.
                damage = regions;
            }
            *previous = Some(frame.clone());
        } else if !damage.is_empty() {
            // Kept up to date even when it is not needed, or the first coarse
            // frame after a run of precise ones would have nothing to compare
            // against and would fall back to a full read.
            *self.previous.lock().unwrap() = Some(frame.clone());
        }

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

    /// What this rung can do, now that it can do most of it.
    ///
    /// Clicking works because we find the target ourselves. Keys go to whatever
    /// holds focus, which the stage handles.
    ///
    /// Typing used to be refused, on the grounds that it needs a focused field
    /// this rung cannot identify. That was half true: it cannot identify a field
    /// *as a field* — OCR reads text, not roles — but it can put focus somewhere
    /// specific, because clicking a located element is exactly how a person
    /// focuses one. So `type` clicks the named element and then sends the text,
    /// which is the same two actions in the same order a person performs, and
    /// fails the same way if the thing clicked was not a text field.
    ///
    /// Scrolling used to be refused here, on the grounds that it has no meaning
    /// without a scrollable container we can identify. That was true of a
    /// *browser*, where the wrong container silently scrolls the document
    /// instead; it is not true of a screen. Scrolling at a point is exactly what
    /// a wheel does, and the thing under that point is whatever the application
    /// decided should be there — which is the same answer a person gets. So it
    /// is offered when the seat can deliver it, and still refused when it
    /// cannot, rather than being reported as done and doing nothing.
    fn caps(&self) -> Caps {
        Caps {
            click: true,
            type_text: true,
            key: true,
            scroll: self.stage.seat().scroll,
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

    async fn relax(&self) -> Result<(), StepError> {
        self.stage.relax(self.window).await
    }

    fn window(&self) -> Option<WindowKey> {
        Some(self.window)
    }

    async fn apply(
        &self,
        step: &Step,
        node: Option<&Node>,
        secondary: Option<&Node>,
    ) -> Result<Option<String>, StepError> {
        // Handled ahead of the match because it is the one verb here that
        // reports a value rather than an effect. Folding it into the match
        // would make every other arm carry an `Option<String>` it never uses.
        if let Step::GetClipboard {} = step {
            return Ok(Some(match self.stage.clipboard_get().await? {
                Some(text) => format!("clipboard: {text:?}"),
                None => "clipboard: empty".to_owned(),
            }));
        }

        match step {
            Step::Click {
                target,
                button,
                count,
                modifiers,
            } => {
                let bounds = self.bounds_of(node, target)?;
                // The centre, computed here from what we detected. The model
                // never supplied a coordinate and never sees one.
                self.stage
                    .pointer(
                        self.window,
                        crate::stage::Pointing::at(bounds)
                            .with_button(button.evdev())
                            .with_count(*count)
                            .with_modifiers(parse_modifiers(modifiers.as_deref())?),
                    )
                    .await?;
                // What we clicked has probably changed, and the rest of the
                // screen has not — damage will say which parts.
                Ok(())
            }
            Step::Hover(target) => {
                let bounds = self.bounds_of(node, target)?;
                self.stage.hover(self.window, bounds).await
            }
            Step::Press { target, button } => {
                let bounds = self.bounds_of(node, target)?;
                self.stage.press(self.window, bounds, button.evdev()).await
            }
            Step::Release { button } => self.stage.release(self.window, button.evdev()).await,
            Step::Drag {
                from,
                to,
                direction,
                distance,
                button,
                modifiers,
                pressure,
                tilt,
            } => {
                let start = self.bounds_of(node, from)?;
                // `to` names the element; `secondary` is that element already
                // resolved and label-checked by `run_program`, exactly as the
                // primary target was.
                let end = self
                    .drag_destination(start, to.as_ref().and(secondary), *direction, *distance)
                    .await?;
                // A pressure asked for makes this a stylus stroke, delivered
                // through the tablet rather than the pointer. Refused outright
                // where the seat has no tablet: falling back would draw the
                // right line at the wrong weight, which reads as the
                // application ignoring pressure rather than as the harness
                // never having sent any.
                match pressure {
                    Some(pressure) => {
                        if !self.stage.seat().tablet {
                            return Err(StepError::Backend(
                                "this display has no stylus, so a drag cannot carry pressure. \
                                 Drop `pressure` to drag with the pointer instead."
                                    .into(),
                            ));
                        }
                        let tilt = tilt.unwrap_or([0.0, 0.0]);
                        self.stage
                            .stylus(self.window, start, end, *pressure, (tilt[0], tilt[1]))
                            .await
                    }
                    None => {
                        self.stage
                            .drag(
                                self.window,
                                start,
                                end,
                                button.evdev(),
                                parse_modifiers(modifiers.as_deref())?,
                            )
                            .await
                    }
                }
            }
            Step::Key(press) => self.stage.key(self.window, press.chord()).await,
            Step::KeyDown(press) => self.stage.key_hold(self.window, press.chord(), true).await,
            Step::KeyUp(press) => self.stage.key_hold(self.window, press.chord(), false).await,
            Step::Type {
                target,
                text,
                clear,
            } => {
                let node = node.ok_or_else(|| {
                    StepError::Backend("typing on a screen surface needs an element".into())
                })?;
                let bounds = node.bounds.ok_or_else(|| {
                    StepError::Backend(format!(
                        "{} has no position on screen — re-observe {}",
                        target.anchor, self.id
                    ))
                })?;
                // Focus first, by clicking where the text is. There is no other
                // way to aim at a field on this rung: the stage delivers text to
                // whatever holds focus, and nothing here knows what that is
                // until we have put it somewhere.
                self.stage
                    .pointer(self.window, crate::stage::Pointing::at(bounds))
                    .await?;
                if *clear {
                    // Select-all then type, which replaces. This is the one
                    // place the rung has to assume a convention rather than
                    // observe a fact — a field that does not honour ctrl+a will
                    // append instead, and that is visible in the next
                    // observation rather than silent.
                    self.stage.key(self.window, "ctrl+a").await?;
                }
                self.stage.text(self.window, text).await
            }
            Step::LongPress(_) => {
                let at = match node.and_then(|node| node.bounds) {
                    Some(bounds) => bounds,
                    None => self.window_bounds().await?,
                };
                self.stage
                    .gesture(
                        self.window,
                        &crate::stage::Gesture::Tap {
                            at,
                            hold_ms: LONG_PRESS_MS,
                        },
                    )
                    .await
            }
            Step::Swipe {
                direction,
                distance,
                ..
            } => {
                let window = self.window_bounds().await?;
                let from = match node.and_then(|node| node.bounds) {
                    Some(bounds) => bounds,
                    None => window,
                };
                // Defaulted from the *window*, not the element: a swipe on a
                // list row means "drag this row", and a row is forty pixels
                // tall, so a distance proportional to it would travel too
                // little to register as anything.
                let span = match direction {
                    crate::program::Direction::Up | crate::program::Direction::Down => {
                        window.height
                    }
                    _ => window.width,
                };
                let travel = distance.unwrap_or(span / 2).min(span) as i32;
                let (dx, dy) = direction.offset(travel);
                let to = crate::model::Rect {
                    x: from.x + dx,
                    y: from.y + dy,
                    width: from.width,
                    height: from.height,
                };
                self.stage
                    .gesture(
                        self.window,
                        &crate::stage::Gesture::Swipe {
                            from,
                            to,
                            duration_ms: SWIPE_MS,
                        },
                    )
                    .await
            }
            Step::Scroll { amount, axis, .. } => {
                // At the named element when there is one, and at the middle of
                // the window otherwise. The fallback is the meaningful case on
                // this rung: a list the OCR read as a column of text has no
                // container to name, and scrolling "the screen" is what the
                // model meant.
                let at = match node.and_then(|node| node.bounds) {
                    Some(bounds) => bounds,
                    None => self.window_bounds().await?,
                };
                self.stage.scroll(self.window, at, *amount, *axis).await
            }
            Step::Pinch { scale, .. } => {
                let at = match node.and_then(|node| node.bounds) {
                    Some(bounds) => bounds,
                    None => self.window_bounds().await?,
                };
                if !scale.is_finite() || *scale <= 0.0 {
                    return Err(StepError::Backend(format!(
                        "a pinch scale of {scale} means nothing; use a positive number, \
                         above 1 to zoom in"
                    )));
                }
                // The starting gap is a fraction of the target rather than a
                // constant, so a pinch on a small element does not put both
                // contacts outside it. Bounded below because two contacts a few
                // pixels apart read as one.
                let from_gap = (at.width.min(at.height) / 2).max(64);
                let to_gap = ((from_gap as f32) * scale).round().clamp(16.0, 4096.0) as u32;
                self.stage
                    .gesture(
                        self.window,
                        &crate::stage::Gesture::Pinch {
                            at,
                            from_gap,
                            to_gap,
                            duration_ms: SWIPE_MS,
                        },
                    )
                    .await
            }
            Step::SetClipboard { text } => self.stage.clipboard_set(text).await,
            // Answered above, before the match.
            Step::GetClipboard {} => Ok(()),
            // A file chooser on this rung is a window like any other, and the
            // agent drives it by typing a path into it. Pretending otherwise
            // would mean inventing a drop target from pixels.
            Step::Upload { .. } => Err(StepError::Backend(
                "a pixel surface has no file input to hand a path to. Open the application's \
                 file chooser and type the path into it — the chooser is a separate window \
                 and appears in `surfaces`."
                    .into(),
            )),
            Step::Dialog { .. } => Err(StepError::Backend(
                "a dialog on this rung is an ordinary window: observe it and click the button \
                 you want, rather than arming an answer in advance."
                    .into(),
            )),
            other => Err(StepError::Backend(format!(
                "a screen surface cannot {:?} — it can click what it can read, and send keys. \
                 If this application has a debugging protocol or an accessibility tree, it \
                 should not be on this rung at all.",
                other.action()
            ))),
        }
        .map(|()| None)
    }
}
