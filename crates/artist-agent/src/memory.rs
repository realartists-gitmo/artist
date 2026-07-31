//! Harness-owned durable memory: the model-facing tool and the write triggers.
//!
//! Memory is deliberately not part of the conversation. Facts live in a store
//! outside the model context and are recorded as session events, so they
//! survive compaction and handoff, and a rewind that masks the write also
//! removes the fact — the same design the todo list uses, for the same reason.
//!
//! Three things write memory automatically, chosen because they are the moments
//! that actually carry signal rather than the moments that are convenient:
//!
//! * a **user correction** — someone stating a durable preference out loud;
//! * a **model decision** stated mid-stream, with its rationale;
//! * a **successful commit** — a completed unit of work.
//!
//! All three are fire-and-forget: none may delay a stream, a tool loop, or the
//! input box.

use artist_memory::{Memory, NewFact, Scope};
use artist_session::{MemoryWritten, Recorder};
use regex::RegexSet;
use rig_agent::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, HookContext, ObservationAction,
    RequestPatch, StepEventKind, TextDelta, ToolResultAction, ToolResultEvent,
};
use rig_core::completion::Document;
use rig_core::tool::PortableTool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// How many facts a single recall returns before injection.
pub const RECALL_LIMIT: usize = 8;

/// Shared handle for the tool, the hooks and the CLI.
///
/// Deliberately session-independent: the store and the embedding model are
/// expensive to open and live for the whole process, while the recorder and
/// session id belong to one session and are supplied per turn — the same split
/// the todo list uses.
#[derive(Clone)]
pub struct MemoryHandle {
    memory: Memory,
    embedder: Option<artist_memory::Embedder>,
}

/// The outcome of a successful write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub id: i64,
    /// The fact this one retired, when it revised rather than added.
    pub replaced: Option<i64>,
}

/// A handle bound to one session's log.
#[derive(Clone)]
pub struct MemoryWriter {
    handle: MemoryHandle,
    recorder: Recorder,
    session: String,
}

impl MemoryHandle {
    pub fn new(memory: Memory, embedder: Option<artist_memory::Embedder>) -> Self {
        Self { memory, embedder }
    }

    /// Bind this handle to a session so writes can be recorded.
    pub fn writer(&self, recorder: Recorder, session: String) -> MemoryWriter {
        MemoryWriter {
            handle: self.clone(),
            recorder,
            session,
        }
    }

    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    pub fn embedder(&self) -> Option<&artist_memory::Embedder> {
        self.embedder.as_ref()
    }

    /// Bring the project's code index up to date, in the background.
    ///
    /// Never awaited on a user-facing path: a first full pass over a large
    /// repository takes close to two hours at the embedder's measured
    /// throughput. Subsequent passes are cheap because chunks are compared by
    /// content hash, so only what actually moved is re-embedded.
    pub fn spawn_code_index(&self, project: std::path::PathBuf) {
        let Some(embedder) = self.embedder.clone() else {
            return;
        };
        let store = self.memory.project().clone();
        tokio::spawn(async move {
            let indexer = artist_memory::Indexer::new(store, embedder);
            match indexer.index_tree(&project).await {
                Ok(report) if report.files_changed > 0 => {
                    eprintln!(
                        "memory: indexed {} changed file(s), +{} -{} chunks",
                        report.files_changed, report.chunks_added, report.chunks_removed
                    );
                }
                Ok(_) => {}
                Err(error) => eprintln!("memory: code indexing stopped: {error}"),
            }
        });
    }

    /// Embed `text` as a query, if an embedder is configured.
    ///
    /// Without one, retrieval degrades to the lexical leg rather than failing:
    /// a missing model should cost recall quality, not break the session.
    pub async fn query_vector(&self, text: &str) -> Vec<f32> {
        match &self.embedder {
            Some(embedder) => embedder.embed_query(text).await.unwrap_or_default(),
            None => Vec::new(),
        }
    }

    async fn document_vector(&self, text: &str) -> Vec<f32> {
        match &self.embedder {
            Some(embedder) => embedder
                .embed_documents(vec![text.to_owned()])
                .await
                .ok()
                .and_then(|mut v| v.pop())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Recall facts relevant to `text`. Errors are swallowed: a memory lookup
    /// must never take down a turn.
    pub async fn recall(&self, text: &str) -> Vec<artist_memory::Hit> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        let vector = self.query_vector(text).await;
        self.memory
            .search(text, &vector, RECALL_LIMIT)
            .await
            .unwrap_or_default()
    }
}

impl MemoryWriter {
    pub fn handle(&self) -> &MemoryHandle {
        &self.handle
    }

    pub async fn recall(&self, text: &str) -> Vec<artist_memory::Hit> {
        self.handle.recall(text).await
    }

    /// Store one fact, recording the write to the session log.
    ///
    /// Returns `None` only when the fact restates one already held, so callers
    /// can tell "stored" from "already known" without a second query.
    ///
    /// A write that *resembles* a stored fact without repeating it revises it:
    /// the new fact is inserted and the old one retired in the same step. That
    /// asymmetry is load-bearing. The admission probe is lexical, and a
    /// negation shares nearly every shingle with what it negates, so treating
    /// every near-duplicate as a duplicate silently discarded exactly the
    /// corrections this path exists to capture — see [`artist_memory::admit`].
    pub async fn remember(
        &self,
        scope: Scope,
        subject: &str,
        predicate: &str,
        object: &str,
        text: &str,
        origin: &str,
    ) -> anyhow::Result<Option<Written>> {
        let store = self.handle.memory.store(scope);
        let embedding = self.handle.document_vector(text).await;
        if embedding.len() != artist_memory::schema::DIM {
            // Without a usable vector the fact would be invisible to the
            // semantic leg and would break the fixed-width column.
            anyhow::bail!("no embedder available; cannot store a fact");
        }
        // Embed first: the candidate probe is ordinary retrieval, and the
        // vector is needed for the write regardless, so nothing is wasted.
        let candidates = store.revision_candidates(text, &embedding, 5).await?;
        let revises = match artist_memory::admit(text, &candidates) {
            artist_memory::Admission::Restates(_) => return Ok(None),
            artist_memory::Admission::Revises(old) => Some(old),
            artist_memory::Admission::Insert => None,
        };
        let ids = store
            .put_facts(&[NewFact {
                subject: subject.to_owned(),
                predicate: predicate.to_owned(),
                object: object.to_owned(),
                text: text.to_owned(),
                embedding,
                source_session: self.session.clone(),
                source_seq: self.recorder.last_committed_seq().unwrap_or(0) as i64,
                origin: origin.to_owned(),
            }])
            .await?;
        let Some(id) = ids.first().copied() else {
            return Ok(None);
        };
        if let Some(old) = revises {
            store.supersede(old, id).await?;
        }
        // Recorded with the supersession attached, so a rewind past this write
        // restores the belief it replaced rather than leaving a gap.
        self.recorder.record(MemoryWritten {
            fact_id: id,
            scope: scope.as_str().to_owned(),
            subject: subject.to_owned(),
            predicate: predicate.to_owned(),
            object: object.to_owned(),
            text: text.to_owned(),
            origin: origin.to_owned(),
            superseded: revises,
        });
        Ok(Some(Written {
            id,
            replaced: revises,
        }))
    }

    /// Replace an existing fact. The old one leaves recall automatically
    /// because every index over `fact` is filtered on `live`.
    pub async fn supersede(
        &self,
        scope: Scope,
        old: i64,
        text: &str,
        origin: &str,
    ) -> anyhow::Result<Option<i64>> {
        let store = self.handle.memory.store(scope);
        let embedding = self.handle.document_vector(text).await;
        if embedding.len() != artist_memory::schema::DIM {
            anyhow::bail!("no embedder available; cannot store a fact");
        }
        let ids = store
            .put_facts(&[NewFact {
                subject: String::new(),
                predicate: String::new(),
                object: text.to_owned(),
                text: text.to_owned(),
                embedding,
                source_session: self.session.clone(),
                source_seq: self.recorder.last_committed_seq().unwrap_or(0) as i64,
                origin: origin.to_owned(),
            }])
            .await?;
        let Some(id) = ids.first().copied() else {
            return Ok(None);
        };
        store.supersede(old, id).await?;
        self.recorder.record(MemoryWritten {
            fact_id: id,
            scope: scope.as_str().to_owned(),
            subject: String::new(),
            predicate: String::new(),
            object: text.to_owned(),
            text: text.to_owned(),
            origin: origin.to_owned(),
            superseded: Some(old),
        });
        Ok(Some(id))
    }
}

// ---------------------------------------------------------------------------
// The model-facing tool
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct MemoryTool {
    handle: MemoryWriter,
}

impl MemoryTool {
    pub fn new(handle: MemoryWriter) -> Self {
        Self { handle }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemoryArgs {
    mode: Option<String>,
    /// `search`: the query. `store`/`supersede`: the fact, as one sentence.
    query: Option<String>,
    text: Option<String>,
    scope: Option<String>,
    /// `supersede`: the id being replaced.
    replaces: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct MemoryError(String);

impl PortableTool for MemoryTool {
    const NAME: &'static str = "memory";
    type Error = MemoryError;
    type Args = MemoryArgs;
    type Output = String;

    fn description(&self) -> String {
        // Tool definitions reach the model through the API's `tools` field, not
        // the system prompt, so all usage guidance has to live here.
        "Durable memory across sessions. mode=search recalls facts; mode=store records one; \
         mode=supersede replaces an existing fact by id when it turns out to be wrong or \
         stale.\n\nStore a fact when you learn something that will still be true next session \
         and is not already obvious from the code: a stated preference, a project constraint, \
         a decision and why it was made, or a correction. Do not store what a file already \
         says, what only matters this turn, or anything you have not actually confirmed.\n\n\
         Write one self-contained sentence — it will be read with no surrounding context. \
         scope=global for things that follow the user between projects, scope=project (the \
         default) for this repository. Memory is kept outside your context and survives \
         compaction and handoff."
            .into()
    }

    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{
            "mode":{"enum":["search","store","supersede"],"default":"search"},
            "query":{"type":"string","description":"What to recall. Required for mode=search."},
            "text":{"type":"string","description":"The fact, as one self-contained sentence. Required for store and supersede."},
            "scope":{"enum":["project","global"],"default":"project"},
            "replaces":{"type":"integer","description":"The fact id being replaced. Required for mode=supersede."}
        },"additionalProperties":false})
    }

    async fn call(&self, args: MemoryArgs) -> Result<String, MemoryError> {
        let scope = match args.scope.as_deref() {
            Some("global") => Scope::Global,
            _ => Scope::Project,
        };
        let mode = args.mode.as_deref().unwrap_or("search");
        match mode {
            "search" => {
                let query = args
                    .query
                    .or(args.text)
                    .ok_or_else(|| MemoryError("mode=search needs a query".into()))?;
                let hits = self.handle.recall(&query).await;
                if hits.is_empty() {
                    return Ok("No memories matched.".into());
                }
                Ok(hits
                    .iter()
                    .map(|h| format!("[{}] ({}) {}", h.id, h.scope.as_str(), h.text.trim()))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "store" => {
                let text = args
                    .text
                    .ok_or_else(|| MemoryError("mode=store needs text".into()))?;
                match self
                    .handle
                    .remember(scope, "", "", &text, &text, "tool")
                    .await
                {
                    // Reporting the retirement matters: without it the model
                    // cannot tell adding a belief from revising one, and would
                    // have no reason to look at what it just replaced.
                    Ok(Some(Written {
                        id,
                        replaced: Some(old),
                    })) => Ok(format!("Stored as [{id}], superseding [{old}].")),
                    Ok(Some(Written { id, .. })) => Ok(format!("Stored as [{id}].")),
                    Ok(None) => Ok("That restates a memory already held; nothing stored.".into()),
                    Err(err) => Err(MemoryError(err.to_string())),
                }
            }
            "supersede" => {
                let text = args
                    .text
                    .ok_or_else(|| MemoryError("mode=supersede needs text".into()))?;
                let old = args
                    .replaces
                    .ok_or_else(|| MemoryError("mode=supersede needs `replaces`".into()))?;
                match self.handle.supersede(scope, old, &text, "tool").await {
                    Ok(Some(id)) => Ok(format!("Replaced [{old}] with [{id}].")),
                    Ok(None) => Ok("Nothing stored.".into()),
                    Err(err) => Err(MemoryError(err.to_string())),
                }
            }
            other => Err(MemoryError(format!("unknown mode {other}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// Write trigger 1: user correction
// ---------------------------------------------------------------------------

/// Phrases that mark the user correcting course.
///
/// This is the highest signal-to-noise moment for memory: a correction is a
/// durable preference being stated out loud. It is a prescreen only — a hit
/// schedules an extraction pass, it does not itself write anything.
///
/// Kept here rather than as a new `MatchTarget` on the rules engine: every TTSR
/// target is streamed model output that can be aborted mid-token, and user text
/// is neither streamed nor abortable, so it would borrow the vocabulary without
/// the semantics.
const CORRECTION_PATTERNS: &[&str] = &[
    r"(?i)\bno,?\s+(actually|wait|i\s+meant)\b",
    r"(?i)\b(don'?t|do not|never)\s+(do|use|add|write|call|put|run)\b",
    r"(?i)\bstop\s+(doing|using|adding)\b",
    r"(?i)\bi\s+(told|asked)\s+you\b",
    r"(?i)\b(instead\s+of|rather\s+than)\b.{0,40}\buse\b",
    r"(?i)\bthat'?s\s+(wrong|not\s+right|not\s+what)\b",
    r"(?i)\b(always|from\s+now\s+on)\b.{0,40}\b(use|prefer|do)\b",
    // A short run of words, not a single one: "we don't vendor dependencies in
    // this repo" puts two between the negation and the scope marker.
    r"(?i)\bwe\s+(?:don'?t|do\s+not)\s+(?:\w+\s+){1,5}(?:here|in\s+this\s+(?:repo|project))\b",
];

/// Longest correction stored verbatim. Past this the message is more likely a
/// task description that happens to contain a correction phrase.
const MAX_CORRECTION_CHARS: usize = 400;

/// Store a user correction, in the background, if the text looks like one.
///
/// The user's own words go in unmodified. Distilling them through the model
/// would cost a call and lose the phrasing that made the preference clear —
/// and the user already said exactly what they meant.
pub(crate) fn capture_correction(writer: MemoryWriter, text: &str) {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_CORRECTION_CHARS {
        return;
    }
    if !CorrectionDetector::new().is_correction(text) {
        return;
    }
    let text = text.to_owned();
    tokio::spawn(async move {
        let _ = writer
            .remember(Scope::Project, "", "", &text, &text, "correction")
            .await;
    });
}

/// Cheap regex prescreen over user text.
#[derive(Clone)]
pub struct CorrectionDetector {
    set: Arc<RegexSet>,
}

impl Default for CorrectionDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrectionDetector {
    pub fn new() -> Self {
        Self {
            // Patterns are compile-time constants; an invalid one is a bug, and
            // an empty set simply never fires.
            set: Arc::new(RegexSet::new(CORRECTION_PATTERNS).unwrap_or_else(|_| RegexSet::empty())),
        }
    }

    pub fn is_correction(&self, text: &str) -> bool {
        !text.trim().is_empty() && self.set.is_match(text)
    }
}

// ---------------------------------------------------------------------------
// Write trigger 3: a successful commit
// ---------------------------------------------------------------------------

/// Recognise a completed `git commit` in a shell tool result.
///
/// A commit is a natural "unit of work finished" marker, and unlike a turn
/// boundary it is low-frequency and coherent. Detecting it from the tool
/// result costs no watcher and no new git integration — the repository has
/// none today beyond root discovery.
pub fn commit_summary(command: &str, output: &str) -> Option<String> {
    if !is_commit_command(command) {
        return None;
    }
    // git prints `[branch abc1234] subject` on success; a failed or empty
    // commit prints neither.
    let line = output
        .lines()
        .find(|line| line.trim_start().starts_with('[') && line.contains(']'))?;
    let subject = line.split_once(']')?.1.trim();
    if subject.is_empty() {
        return None;
    }
    Some(subject.to_owned())
}

fn is_commit_command(command: &str) -> bool {
    let normalized = command.trim();
    normalized.contains("git commit") && !normalized.contains("--dry-run")
}

// ---------------------------------------------------------------------------
// Write trigger 2 + read channel 3: the agent hook
// ---------------------------------------------------------------------------

/// Phrases that mark the model committing to a decision with a reason.
///
/// Deliberately narrow: this fires mid-stream on every turn, so a loose pattern
/// would bury the store in restatements of things the transcript already says.
const DECISION_PATTERNS: &[&str] = &[
    r"(?i)\b(I'?ll|I\s+will|going\s+to)\s+use\s+\w+\s+(because|since|so\s+that)\b",
    r"(?i)\b(chose|choosing|picked|opting\s+for)\s+\w+\s+(over|instead\s+of)\s+\w+\b",
    r"(?i)\bthe\s+reason\s+.{0,60}\bis\s+that\b",
    r"(?i)\bwon'?t\s+work\s+because\b",
];

/// Shared state between the streaming hook and the completion-call injector.
struct HookState {
    /// Tail of the current model turn, bounded like the rules matcher's window.
    window: String,
    /// Recalled facts waiting to be injected on the next completion call.
    pending: Vec<artist_memory::Hit>,
    /// Recall fires at most once per user turn — the hook is constructed fresh
    /// by each `stream_chat_with` call, so this needs no explicit reset. Bounded
    /// deliberately: a long tool loop restating the same decision should not
    /// inject the same facts repeatedly.
    fired_this_turn: bool,
}

/// Matching window in bytes, mirroring the stream rules default.
const WINDOW_BYTES: usize = 4096;
/// Evaluate only once this many new bytes accumulate, as the rules matcher does.
const COALESCE_BYTES: usize = 64;

/// Observes the model's own output and turns it into memory reads and writes.
///
/// This is the stream-rules mechanism pointed at a different purpose, with one
/// critical difference: it **never aborts**. Stream rules terminate the run to
/// keep a mistake out of context; memory only ever adds context, so a match
/// here schedules work and returns `keep`. The retrieval it kicks off runs in
/// the background and is collected by `on_completion_call`, which is why it
/// never blocks the stream — the model is still generating while it runs.
/// Registered unconditionally and inert when `handle` is `None`, so the agent
/// builder stays one straight chain rather than branching on configuration.
#[derive(Clone)]
pub(crate) struct MemoryHook {
    handle: Option<MemoryWriter>,
    decisions: Arc<RegexSet>,
    state: Arc<std::sync::Mutex<HookState>>,
    /// Facts recalled in the background, ready for the next completion call.
    inbox: Arc<std::sync::Mutex<Vec<artist_memory::Hit>>>,
    auto_write: bool,
}

impl MemoryHook {
    pub fn new(handle: Option<MemoryWriter>, auto_write: bool) -> Self {
        Self {
            handle,
            decisions: Arc::new(
                RegexSet::new(DECISION_PATTERNS).unwrap_or_else(|_| RegexSet::empty()),
            ),
            state: Arc::new(std::sync::Mutex::new(HookState {
                window: String::new(),
                pending: Vec::new(),
                fired_this_turn: false,
            })),
            inbox: Arc::new(std::sync::Mutex::new(Vec::new())),
            auto_write,
        }
    }

    fn enabled(&self) -> bool {
        self.handle.is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HookState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Feed streamed text; returns an excerpt when recall should be triggered.
    fn observe(&self, delta: &str) -> Option<String> {
        let mut state = self.lock();
        if state.fired_this_turn {
            return None;
        }
        state.window.push_str(delta);
        if state.window.len() > WINDOW_BYTES {
            let cut = state.window.len() - WINDOW_BYTES;
            let cut = floor_char_boundary(&state.window, cut);
            state.window.drain(..cut);
        }
        if delta.len() < COALESCE_BYTES && !delta.contains('\n') {
            return None;
        }
        if !self.decisions.is_match(&state.window) {
            return None;
        }
        state.fired_this_turn = true;
        Some(state.window.clone())
    }

    /// Kick off a recall without blocking the stream.
    ///
    /// Fire-and-forget is what makes this safe on the hot path: the model is
    /// still generating, so the lookup has a natural window to finish before
    /// the next completion call collects it.
    fn schedule_recall(&self, excerpt: String) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let inbox = Arc::clone(&self.inbox);
        tokio::spawn(async move {
            let hits = handle.recall(&excerpt).await;
            if hits.is_empty() {
                return;
            }
            let mut slot = inbox.lock().unwrap_or_else(|e| e.into_inner());
            slot.extend(hits);
        });
    }

    /// Store a fact from a completed commit, in the background.
    fn schedule_commit_write(&self, subject: String) {
        if !self.auto_write {
            return;
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        tokio::spawn(async move {
            let text = format!("Committed: {subject}");
            let _ = handle
                .remember(Scope::Project, "", "", &text, &text, "commit")
                .await;
        });
    }

    /// Drain whatever recall has landed, deduplicated by fact id.
    fn take_recalled(&self) -> Vec<artist_memory::Hit> {
        let mut inbox = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        let mut hits: Vec<_> = inbox.drain(..).collect();
        let mut state = self.lock();
        hits.append(&mut state.pending);
        let mut seen = std::collections::BTreeSet::new();
        hits.retain(|h| seen.insert((h.scope.as_str(), h.id)));
        hits
    }
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

impl AgentHook for MemoryHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        // Inert when memory is off, so a disabled subsystem costs nothing on
        // the streaming path.
        self.enabled()
            && matches!(
                kind,
                StepEventKind::TextDelta
                    | StepEventKind::CompletionCall
                    | StepEventKind::ToolResult
            )
    }

    /// Read channel 3. Injects outside ordinary history, so recalled facts
    /// survive compaction and handoff by construction — the same property the
    /// session-persistent stream rules rely on.
    async fn on_completion_call(
        &self,
        _context: &HookContext,
        _event: CompletionCallEvent<'_>,
    ) -> CompletionCallAction {
        let hits = self.take_recalled();
        if hits.is_empty() {
            return CompletionCallAction::continue_run();
        }
        let text = artist_memory::render(&hits);
        if text.is_empty() {
            return CompletionCallAction::continue_run();
        }
        CompletionCallAction::patch(RequestPatch::new().extra_context([Document {
            id: "artist-memory".to_owned(),
            text,
            additional_props: Default::default(),
        }]))
    }

    /// Write trigger 2. Never aborts: stream rules terminate a run to keep a
    /// mistake out of context, but memory only ever adds context, so a match
    /// here schedules work and lets the turn continue untouched.
    async fn on_text_delta(
        &self,
        _context: &HookContext,
        event: TextDelta<'_>,
    ) -> ObservationAction {
        if let Some(excerpt) = self.observe(event.delta) {
            self.schedule_recall(excerpt);
        }
        ObservationAction::continue_run()
    }

    /// Write trigger 3. A commit is a completed unit of work — low frequency
    /// and coherent, unlike a turn boundary.
    async fn on_tool_result(
        &self,
        _context: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> ToolResultAction {
        if self.enabled() && event.tool_name == "bash" {
            // `args` is the raw JSON argument string; the substring check works
            // on it directly, and scoping to the shell tool keeps this off every
            // other tool's result path.
            let rendered = event.presentation.render();
            if let Some(subject) = commit_summary(event.args, &rendered) {
                self.schedule_commit_write(subject);
            }
        }
        ToolResultAction::keep()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correction_detector_fires_on_pushback() {
        let detector = CorrectionDetector::new();
        for text in [
            "no, actually use the Edit tool",
            "don't use python heredocs for that",
            "I told you to stage on Gortnite",
            "instead of sed, use the harness edit tools",
            "from now on prefer ratatui widgets",
            "we don't vendor dependencies in this repo",
        ] {
            assert!(detector.is_correction(text), "should fire on {text:?}");
        }
    }

    #[test]
    fn correction_detector_ignores_ordinary_requests() {
        let detector = CorrectionDetector::new();
        for text in [
            "add a test for the compaction planner",
            "what does the rules engine do?",
            "run the tests and show me the output",
            "",
        ] {
            assert!(!detector.is_correction(text), "should not fire on {text:?}");
        }
    }

    #[test]
    fn commit_summary_extracts_the_subject() {
        let output = "[Gortnite 1a2b3c4] feat(memory): add the store\n 3 files changed";
        assert_eq!(
            commit_summary("git commit -m 'feat(memory): add the store'", output).as_deref(),
            Some("feat(memory): add the store")
        );
    }

    #[test]
    fn commit_summary_ignores_non_commits_and_dry_runs() {
        let output = "[Gortnite 1a2b3c4] something";
        assert!(commit_summary("git status", output).is_none());
        assert!(commit_summary("git commit --dry-run", output).is_none());
        assert!(commit_summary("git commit", "nothing to commit").is_none());
    }

    fn decision_set() -> RegexSet {
        RegexSet::new(DECISION_PATTERNS).unwrap()
    }

    #[test]
    fn decision_patterns_fire_on_stated_rationale() {
        let set = decision_set();
        for text in [
            "I'll use RocksDB because it has the MultiGet batch path",
            "choosing rten over candle since candle pulls a C dependency",
            "the reason we keep the preamble stable is that it is a cache prefix",
            "that won't work because the index has no live filter",
        ] {
            assert!(set.is_match(text), "should match {text:?}");
        }
    }

    #[test]
    fn decision_patterns_ignore_narration() {
        // This hook runs on every streamed turn, so a loose pattern would bury
        // the store in restatements the transcript already holds.
        let set = decision_set();
        for text in [
            "Let me read the file first.",
            "I'll run the tests now.",
            "Here is what the function does.",
            "Looking at crates/artist-agent/src/lib.rs",
        ] {
            assert!(!set.is_match(text), "should not match {text:?}");
        }
    }
}
