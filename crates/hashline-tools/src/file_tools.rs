use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use xxhash_rust::xxh3::xxh3_64;

fn drift_fingerprint(data: &[u8]) -> u64 {
    xxh3_64(data)
}

fn normalize_replacement_content(content: &str) -> &str {
    content
        .strip_suffix("\r\n")
        .or_else(|| content.strip_suffix('\n'))
        .unwrap_or(content)
}

/// Re-terminate every line of inserted `content` with the file's dominant
/// `newline` (and guarantee a trailing one), so a bare `\n` in the supplied
/// content can't introduce mixed endings into a CRLF file. Empty content
/// inserts a single blank line, matching the previous behaviour.
fn normalize_insertion(content: &str, newline: &str) -> String {
    if content.is_empty() {
        return newline.to_owned();
    }
    let mut normalized = String::new();
    for line in content.split_inclusive('\n') {
        normalized.push_str(line.trim_end_matches(['\r', '\n']));
        normalized.push_str(newline);
    }
    normalized
}

/// Byte-range information for a single line in the original content.
/// The terminator (\n or \r\n) is kept separate so Replace/Insert
/// operations can preserve the original line structure.
#[derive(Debug, Clone, Copy)]
struct LineRange {
    /// Byte offset of the first character of line content.
    content_start: usize,
    /// Byte offset of the first character after the line content
    /// (before the line terminator).
    content_end: usize,
    /// Byte offset of the first character after the line terminator
    /// (i.e. start of next line, or end of content for the last line).
    line_end: usize,
}

fn line_byte_ranges(content: &str) -> Vec<LineRange> {
    let mut ranges = Vec::new();
    let mut cursor = 0;
    let bytes = content.as_bytes();
    while cursor < bytes.len() {
        let content_start = cursor;
        // Find the first \n (or end of content)
        while cursor < bytes.len() && bytes[cursor] != b'\n' {
            cursor += 1;
        }
        let mut content_end = cursor;
        // If the byte before \n is \r, exclude it from content
        if content_end > content_start && content_end > 0 && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        // Skip \n if present
        if cursor < bytes.len() {
            cursor += 1; // skip \n
        }
        let line_end = cursor;
        ranges.push(LineRange {
            content_start,
            content_end,
            line_end,
        });
    }
    ranges
}

#[derive(Debug, Clone)]
struct LineInfo {
    anchor: String,
    content: String,
}

#[derive(Debug, Clone)]
struct FileView {
    lines: Vec<LineInfo>,
}

/// Enough about the last view handed to the model to tell, cheaply, whether
/// disk still agrees with what it believes.
///
/// The hash is the authority; `mtime` and `len` exist only so the common answer
/// — "nothing moved" — costs a stat rather than a read. That matters because
/// this is checked after every tool call.
#[derive(Debug, Clone)]
struct ReadStamp {
    content_fingerprint: u64,
    mtime: Option<std::time::SystemTime>,
    len: u64,
    /// Ordering only, so a drift report can spend its budget on whatever the
    /// model touched most recently. A counter rather than a clock: this needs
    /// to be monotonic, and wall time on a shared worktree is not.
    touched: u64,
}

/// What became of a file since the model last looked at it.
#[derive(Debug, Clone)]
pub enum DriftKind {
    /// Still there, different content. Carries the anchors either side, so the
    /// report can say which handles died and what replaced them.
    Modified {
        before: Vec<AnchoredLine>,
        after: Vec<AnchoredLine>,
        before_text: String,
        after_text: String,
    },
    /// Gone. Distinct from modified because "re-read it" is not the advice —
    /// the model is holding anchors into a file that no longer exists.
    Deleted,
    /// There, but we could not look: permissions, a partial write, a directory
    /// where a file used to be.
    Unreadable(String),
}

/// One file whose content no longer matches what the model was shown.
#[derive(Debug, Clone)]
pub struct Drift {
    pub path: String,
    pub kind: DriftKind,
    /// The `touched` counter from the stamp, for recency ordering.
    pub touched: u64,
    /// Which actor wrote it last, when that is known and is not us. A change
    /// from another session is a coordination signal; one from nowhere in
    /// particular is usually the user's editor.
    pub writer: Option<String>,
}

/// A path to examine, and what it looked like when the model last saw it.
///
/// Handed out so the stat can happen without the manager's lock held — see
/// `FileCoordinator::drifted`.
#[derive(Debug, Clone)]
pub struct DriftCandidate {
    pub path: String,
    pub touched: u64,
    mtime: Option<std::time::SystemTime>,
    len: u64,
}

impl DriftCandidate {
    /// Has this file moved since the model saw it, judged by stat alone?
    ///
    /// Deliberately errs towards "yes": a stat that fails, or a filesystem with
    /// coarse mtime granularity, sends the path on to the hash check rather
    /// than silently passing. False positives cost one read; false negatives
    /// cost the model a wasted edit.
    pub fn may_have_moved(&self) -> bool {
        match std::fs::metadata(&self.path) {
            Ok(meta) => meta.len() != self.len || meta.modified().ok() != self.mtime,
            Err(_) => true,
        }
    }
}

impl FileView {
    fn from_text(text: &str, path: &Path) -> Self {
        let raw_lines: Vec<&str> = text.lines().collect();
        let identities = artist_ast::anchors::line_anchor_identities(path, text);
        assert_eq!(identities.len(), raw_lines.len());
        let anchors = crate::semantic_anchors::shortest_live_anchors(&identities);
        let lines = raw_lines
            .into_iter()
            .zip(identities)
            .zip(anchors)
            .map(|((content, _identity), anchor)| LineInfo {
                anchor,
                content: content.to_owned(),
            })
            .collect();
        Self { lines }
    }
}

#[derive(Debug, Clone)]
pub struct ReadFileRequest {
    pub path: String,
    pub start_line: usize,
    pub max_lines: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchoredLine {
    pub line_number: usize,
    pub anchor: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ReadFileResult {
    pub path: String,
    pub content: String,
    pub lines: Vec<AnchoredLine>,
    pub total_lines: usize,
}

#[derive(Debug, Clone)]
pub struct WriteFileRequest {
    pub path: String,
    pub content: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct WriteFileResult {
    pub path: String,
    pub content: String,
    pub lines: Vec<AnchoredLine>,
    pub total_lines: usize,
}

#[derive(Debug, Clone)]
pub enum EditOperation {
    Delete {
        anchor: String,
        end_anchor: Option<String>,
    },
    Replace {
        anchor: String,
        end_anchor: Option<String>,
        content: String,
    },
    InsertBefore {
        anchor: String,
        content: String,
    },
    InsertAfter {
        anchor: String,
        content: String,
    },
}

#[derive(Debug, Clone)]
pub struct EditRequest {
    pub path: String,
    pub operations: Vec<EditOperation>,
}

#[derive(Debug, Clone)]
pub struct EditResult {
    pub path: String,
    pub content: String,
    pub before_lines: Vec<AnchoredLine>,
    pub lines: Vec<AnchoredLine>,
    pub total_lines: usize,
}

#[derive(Debug, Clone)]
pub struct FileToolConfig {
    pub workspace_root: Option<PathBuf>,
    pub allow_outside_workspace: bool,
    pub follow_symlinks: bool,
}

impl Default for FileToolConfig {
    fn default() -> Self {
        Self {
            workspace_root: None,
            allow_outside_workspace: true,
            follow_symlinks: true,
        }
    }
}

#[derive(Clone)]
pub struct FileToolManager {
    config: FileToolConfig,
    // Drift/read tracking is intentionally behaviorally isolated from anchor identity.
    last_read_view: HashMap<String, FileView>,
    read_stamps: HashMap<String, ReadStamp>,
    touch_counter: u64,
}

impl Default for FileToolManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FileToolManager {
    pub fn new() -> Self {
        Self::with_config(FileToolConfig::default())
    }

    pub fn with_config(config: FileToolConfig) -> Self {
        Self {
            config,
            last_read_view: HashMap::new(),
            read_stamps: HashMap::new(),
            touch_counter: 0,
        }
    }

    /// Record what the model was just shown for `path`, and what disk looked
    /// like at that moment.
    ///
    /// Called wherever `last_read_view` is written. Keeping the two together in
    /// one method is the point: a view recorded without a stamp is a file the
    /// drift check silently stops watching.
    fn remember_view(&mut self, path: &str, view: FileView, text: &str) {
        self.last_read_view.insert(path.to_owned(), view);
        let meta = std::fs::metadata(path).ok();
        self.touch_counter += 1;
        self.read_stamps.insert(
            path.to_owned(),
            ReadStamp {
                content_fingerprint: drift_fingerprint(text.as_bytes()),
                mtime: meta.as_ref().and_then(|meta| meta.modified().ok()),
                len: meta.as_ref().map(|meta| meta.len()).unwrap_or(0),
                touched: self.touch_counter,
            },
        );
    }

    /// Every path the model has been shown, with enough to stat-check it.
    ///
    /// Phase one of the drift check. Returns immediately and does no I/O, so
    /// the caller can release the manager lock before touching the filesystem.
    pub fn drift_candidates(&self) -> Vec<DriftCandidate> {
        self.read_stamps
            .iter()
            .map(|(path, stamp)| DriftCandidate {
                path: path.clone(),
                touched: stamp.touched,
                mtime: stamp.mtime,
                len: stamp.len,
            })
            .collect()
    }

    /// Phase three: confirm and describe drift for paths that failed the stat
    /// prefilter.
    ///
    /// Reads and reconciles, so it needs the lock — but only ever runs for
    /// files that actually moved, which is why holding it here is affordable.
    /// A path that turns out to match after all returns nothing: mtime moving
    /// without content changing is ordinary (a touch, a rewrite of identical
    /// bytes, a checkout that restored what was there).
    pub fn describe_drift(&mut self, candidates: &[DriftCandidate]) -> Vec<Drift> {
        let mut drifts = Vec::new();
        for candidate in candidates {
            let Some(stamp) = self.read_stamps.get(&candidate.path).cloned() else {
                continue;
            };
            let path = Path::new(&candidate.path);
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.read_stamps.remove(&candidate.path);
                    drifts.push(Drift {
                        path: candidate.path.clone(),
                        kind: DriftKind::Deleted,
                        touched: stamp.touched,
                        writer: None,
                    });
                    continue;
                }
                Err(error) => {
                    drifts.push(Drift {
                        path: candidate.path.clone(),
                        kind: DriftKind::Unreadable(error.to_string()),
                        touched: stamp.touched,
                        writer: None,
                    });
                    continue;
                }
            };

            if drift_fingerprint(text.as_bytes()) == stamp.content_fingerprint {
                // Same bytes after all. Restamp so the next check does not keep
                // re-reading a file whose mtime merely moved.
                let view = self.build_view(&text, path);
                self.remember_view(&candidate.path, view, &text);
                continue;
            }

            let before_view = self.last_read_view.get(&candidate.path).cloned();
            let before_text = before_view
                .as_ref()
                .map(|view| {
                    view.lines
                        .iter()
                        .map(|line| line.content.clone())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let before = before_view
                .as_ref()
                .map(Self::render_view)
                .unwrap_or_default();

            let after_view = self.build_view(&text, path);
            // Recompute deterministic anchors for the changed live file.
            let after = Self::render_view(&after_view);
            self.remember_view(&candidate.path, after_view, &text);

            drifts.push(Drift {
                path: candidate.path.clone(),
                kind: DriftKind::Modified {
                    before,
                    after,
                    before_text,
                    after_text: text,
                },
                touched: stamp.touched,
                writer: None,
            });
        }
        drifts
    }

    fn build_view(&self, text: &str, path: &Path) -> FileView {
        FileView::from_text(text, path)
    }

    pub fn config(&self) -> &FileToolConfig {
        &self.config
    }

    pub fn normalized_path(&self, path: &str) -> Result<String> {
        normalize_path(path, &self.config)
    }

    pub fn forget_path(&mut self, path: &str) -> Result<()> {
        let normalized = normalize_path(path, &self.config)?;
        self.last_read_view.remove(&normalized);
        self.read_stamps.remove(&normalized);
        Ok(())
    }

    fn render_view(view: &FileView) -> Vec<AnchoredLine> {
        view.lines
            .iter()
            .enumerate()
            .map(|(index, line)| AnchoredLine {
                line_number: index + 1,
                anchor: line.anchor.clone(),
                text: line.content.clone(),
            })
            .collect()
    }

    pub async fn read_file(&mut self, request: ReadFileRequest) -> Result<ReadFileResult> {
        let norm = normalize_path(&request.path, &self.config)?;
        let path = Path::new(&norm);
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", request.path))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let view = self.build_view(&content, path);
        self.remember_view(&norm, view.clone(), &content);

        // Clamped: an offset past the end is a request for nothing, not a
        // reason to slice out of range.
        let start = request.start_line.saturating_sub(1).min(lines.len());
        let end = match request.max_lines {
            Some(max) => (start + max).min(lines.len()),
            None => lines.len(),
        };

        let visible_anchors: Vec<String> =
            view.lines.iter().map(|line| line.anchor.clone()).collect();
        let mut rendered = String::new();
        let mut structured = Vec::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_idx = start + i;
            if view.lines.get(line_idx).is_some() {
                let anchor = visible_anchors[line_idx].clone();
                rendered.push_str(&format!("{}: {}\n", anchor, line));
                structured.push(AnchoredLine {
                    line_number: line_idx + 1,
                    anchor,
                    text: (*line).to_owned(),
                });
            }
        }

        Ok(ReadFileResult {
            path: request.path,
            content: rendered,
            lines: structured,
            total_lines,
        })
    }

    pub async fn write_file(&mut self, request: WriteFileRequest) -> Result<WriteFileResult> {
        let norm = normalize_path(&request.path, &self.config)?;
        let path = Path::new(&norm);

        if !request.overwrite && path.exists() {
            bail!(
                "file already exists: {} (use overwrite=true to overwrite)",
                request.path
            );
        }

        let dir = path.parent().unwrap_or(Path::new("."));
        tokio::fs::create_dir_all(dir).await?;

        let tmp_path = {
            let mut p = path.as_os_str().to_owned();
            p.push(".tmp");
            Path::new(&p).to_owned()
        };

        tokio::fs::write(&tmp_path, &request.content).await?;
        tokio::fs::rename(&tmp_path, path).await?;

        let lines: Vec<&str> = request.content.lines().collect();
        let view = self.build_view(&request.content, path);
        self.remember_view(&norm, view.clone(), &request.content);

        let visible_anchors: Vec<String> =
            view.lines.iter().map(|line| line.anchor.clone()).collect();
        let mut rendered = String::new();
        let mut structured = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if view.lines.get(i).is_some() {
                let anchor = visible_anchors[i].clone();
                rendered.push_str(&format!("{}: {}\n", anchor, line));
                structured.push(AnchoredLine {
                    line_number: i + 1,
                    anchor,
                    text: (*line).to_owned(),
                });
            }
        }

        Ok(WriteFileResult {
            path: request.path,
            content: rendered,
            lines: structured,
            total_lines: lines.len(),
        })
    }

    pub async fn preview_edit_file(&self, request: EditRequest) -> Result<EditResult> {
        let mut preview = self.clone();
        preview.edit_file_inner(request, false).await
    }

    pub async fn edit_file(&mut self, request: EditRequest) -> Result<EditResult> {
        self.edit_file_inner(request, true).await
    }

    async fn edit_file_inner(&mut self, request: EditRequest, apply: bool) -> Result<EditResult> {
        let norm = normalize_path(&request.path, &self.config)?;
        let path = Path::new(&norm);
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", request.path))?;

        // 1. Snapshot: byte ranges for each line in the ORIGINAL content
        //    (preserves CRLF, tab characters, unrelated bytes).
        let line_ranges = line_byte_ranges(&content);
        let snapshot_view = self.build_view(&content, path);
        let snapshot_anchors: Vec<String> = snapshot_view
            .lines
            .iter()
            .map(|line| line.anchor.clone())
            .collect();
        let before_lines = content
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                snapshot_view.lines.get(index).map(|_| AnchoredLine {
                    line_number: index + 1,
                    anchor: snapshot_anchors[index].clone(),
                    text: line.to_owned(),
                })
            })
            .collect::<Vec<_>>();

        // Resolve every operation's opaque anchor against the original snapshot.
        // Matching is exact: no trimming, case folding, Unicode normalization, or fuzzing.
        #[derive(Clone, Debug)]
        struct ResolvedOp {
            byte_start: usize,
            byte_end: usize,
            kind: OpKind,
            /// Original index in request.operations for stable tie-breaking.
            op_idx: usize,
            /// Snapshot line index this op targets (for conflict detection).
            line_idx: usize,
        }

        #[derive(Clone, Debug)]
        enum OpKind {
            Delete,
            Replace { content: String },
            InsertBefore { content: String },
            InsertAfter { content: String },
        }

        let resolve_line_idx = |anchor: &str| -> anyhow::Result<usize> {
            snapshot_view
                .lines
                .iter()
                .position(|line| line.anchor == anchor)
                .ok_or_else(|| anyhow::anyhow!(
                    "anchor '{}' does not exactly resolve to a live occurrence in '{}'; re-read the file for current anchors",
                    anchor,
                    request.path
                ))
        };
        let mut resolved: Vec<ResolvedOp> = Vec::new();

        for (op_idx, operation) in request.operations.iter().enumerate() {
            match operation {
                EditOperation::Delete { anchor, end_anchor } => {
                    let start_idx = resolve_line_idx(anchor)?;
                    let end_idx = if let Some(end_anchor) = end_anchor {
                        resolve_line_idx(end_anchor)?
                    } else {
                        start_idx
                    };
                    if end_idx < start_idx {
                        bail!("delete range end precedes start in '{}'", request.path);
                    }
                    let start = line_ranges[start_idx];
                    let end = line_ranges[end_idx];
                    resolved.push(ResolvedOp {
                        byte_start: start.content_start,
                        byte_end: end.line_end,
                        line_idx: start_idx,
                        kind: OpKind::Delete,
                        op_idx,
                    });
                }
                EditOperation::Replace {
                    anchor,
                    end_anchor,
                    content: new_content,
                } => {
                    let start_idx = resolve_line_idx(anchor)?;
                    let end_idx = if let Some(end_anchor) = end_anchor {
                        resolve_line_idx(end_anchor)?
                    } else {
                        start_idx
                    };
                    if end_idx < start_idx {
                        bail!("replace range end precedes start in '{}'", request.path);
                    }
                    let start = line_ranges[start_idx];
                    let end = line_ranges[end_idx];
                    resolved.push(ResolvedOp {
                        byte_start: start.content_start,
                        byte_end: end.content_end,
                        line_idx: start_idx,
                        kind: OpKind::Replace {
                            content: new_content.clone(),
                        },
                        op_idx,
                    });
                }
                EditOperation::InsertBefore {
                    anchor,
                    content: new_content,
                } => {
                    let idx = resolve_line_idx(anchor)?;
                    let lr = line_ranges[idx];
                    resolved.push(ResolvedOp {
                        byte_start: lr.content_start,
                        byte_end: lr.content_start,
                        line_idx: idx,
                        kind: OpKind::InsertBefore {
                            content: new_content.clone(),
                        },
                        op_idx,
                    });
                }
                EditOperation::InsertAfter {
                    anchor,
                    content: new_content,
                } => {
                    let idx = resolve_line_idx(anchor)?;
                    let lr = line_ranges[idx];
                    resolved.push(ResolvedOp {
                        byte_start: lr.line_end,
                        byte_end: lr.line_end,
                        line_idx: idx,
                        kind: OpKind::InsertAfter {
                            content: new_content.clone(),
                        },
                        op_idx,
                    });
                }
            }
        }

        // 4. Validate: all operations resolved (no write before first resolve failure).
        //    Conflict detection for same-line-index operations.
        for i in 0..resolved.len() {
            for j in (i + 1)..resolved.len() {
                let byte_i = &resolved[i];
                let byte_j = &resolved[j];
                // Check if targeting the same snapshot line index
                if byte_i.line_idx == byte_j.line_idx {
                    let compatible = matches!(
                        (&byte_i.kind, &byte_j.kind),
                        (OpKind::InsertBefore { .. }, OpKind::InsertAfter { .. })
                            | (OpKind::InsertAfter { .. }, OpKind::InsertBefore { .. })
                    );
                    if !compatible {
                        bail!(
                            "Conflicting operations both target line {} in '{}'",
                            byte_i.line_idx + 1,
                            request.path,
                        );
                    }
                }
            }
        }

        // 5. Reject overlapping non-empty byte ranges for Delete/Replace, and
        //    reject an insertion whose position falls *inside* another op's
        //    Delete/Replace range. The latter is the subtle case: an insert is
        //    an empty byte range, so it never trips the overlap check, yet
        //    back-to-front application (step 6) runs the interior insert first,
        //    growing the buffer, after which the range op drains stale snapshot
        //    offsets and cuts the wrong bytes — silent corruption. Insertions at
        //    a range boundary (== byte_start or == byte_end) stay legal: those
        //    are the intended "insert immediately before/after the block" cases.
        for i in 0..resolved.len() {
            for j in (i + 1)..resolved.len() {
                let a = &resolved[i];
                let b = &resolved[j];
                let a_empty = a.byte_start == a.byte_end;
                let b_empty = b.byte_start == b.byte_end;
                match (a_empty, b_empty) {
                    // Two insertions never corrupt each other (handled by the
                    // same-line-index compatibility check in step 4).
                    (true, true) => {}
                    // Two ranges: reject any overlap.
                    (false, false) => {
                        let (al, ar) = if a.byte_start <= b.byte_start {
                            (a, b)
                        } else {
                            (b, a)
                        };
                        if al.byte_end > ar.byte_start {
                            bail!(
                                "Overlapping byte ranges: [{}, {}) and [{}, {}) in '{}'",
                                a.byte_start,
                                a.byte_end,
                                b.byte_start,
                                b.byte_end,
                                request.path,
                            );
                        }
                    }
                    // One insertion, one range: reject only if the insert sits
                    // strictly inside the range.
                    _ => {
                        let (ins, range) = if a_empty { (a, b) } else { (b, a) };
                        if range.byte_start < ins.byte_start && ins.byte_start < range.byte_end {
                            bail!(
                                "Insertion at byte {} falls inside the range [{}, {}) of another operation in '{}'; split this into separate edits so the target is unambiguous",
                                ins.byte_start,
                                range.byte_start,
                                range.byte_end,
                                request.path,
                            );
                        }
                    }
                }
            }
        }

        // 6. Apply operations back-to-front (descending byte_start) so
        //    that earlier positions are not disturbed by later edits.
        //
        //    At an equal byte offset, a non-empty replace/delete must run before
        //    an insertion. This occurs naturally at line boundaries: insert_after
        //    on line N and replace/delete on line N+1 share the same snapshot
        //    offset. Applying the insertion first would shift the bytes beneath
        //    the still-snapshot-relative replacement and corrupt the inserted text.
        //
        //    Equal-position insertions run in reverse request order because each
        //    insert_str at the same index prepends to the prior insertion; reversing
        //    application preserves their original request order in the final file.
        resolved.sort_by(|a, b| {
            b.byte_start.cmp(&a.byte_start).then_with(|| {
                let a_empty = a.byte_start == a.byte_end;
                let b_empty = b.byte_start == b.byte_end;
                a_empty.cmp(&b_empty).then_with(|| {
                    if a_empty && b_empty {
                        b.op_idx.cmp(&a.op_idx)
                    } else {
                        a.op_idx.cmp(&b.op_idx)
                    }
                })
            })
        });

        // Inserted lines take the file's dominant terminator so a CRLF file
        // doesn't end up with mixed line endings.
        let newline = if content.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut result = content;
        for op in &resolved {
            match &op.kind {
                OpKind::Delete => {
                    result.drain(op.byte_start..op.byte_end);
                }
                OpKind::Replace { content, .. } => {
                    // Replace only the content portion; the original terminator
                    // at [op.byte_end..line_end) stays in place naturally.
                    result.replace_range(
                        op.byte_start..op.byte_end,
                        normalize_replacement_content(content),
                    );
                }
                OpKind::InsertBefore { content } => {
                    result.insert_str(op.byte_start, &normalize_insertion(content, newline));
                }
                OpKind::InsertAfter { content } => {
                    // On the final line without a trailing terminator, the
                    // insertion point is the end of that line's text with no
                    // separator — add one so old and new don't concatenate.
                    let separator = if op.byte_start > 0 && !result[..op.byte_start].ends_with('\n')
                    {
                        newline
                    } else {
                        ""
                    };
                    let insertion = format!("{separator}{}", normalize_insertion(content, newline));
                    result.insert_str(op.byte_start, &insertion);
                }
            }
        }

        // Write the final result atomically for an applied edit. Preview uses
        // the same exact-anchor resolution against a cloned manager but leaves
        // the filesystem untouched.
        if apply {
            let dir = path.parent().unwrap_or(Path::new("."));
            tokio::fs::create_dir_all(dir).await?;

            use std::io::Write;
            let permissions = std::fs::metadata(path)
                .ok()
                .map(|metadata| metadata.permissions());
            let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
            temporary.write_all(result.as_bytes())?;
            temporary.as_file().sync_all()?;
            if let Some(permissions) = permissions {
                temporary.as_file().set_permissions(permissions)?;
            }
            temporary.persist(path).map_err(|error| error.error)?;
            if let Ok(directory) = std::fs::File::open(dir) {
                let _ = directory.sync_all();
            }
        }
        // Recompute complete stateless occurrence identities and shortest live prefixes.
        let final_view = self.build_view(&result, path);
        self.remember_view(&norm, final_view.clone(), &result);
        let visible_anchors: Vec<String> = final_view
            .lines
            .iter()
            .map(|line| line.anchor.clone())
            .collect();

        let result_lines: Vec<&str> = result.lines().collect();
        let mut rendered = String::new();
        let mut structured = Vec::new();
        for (i, line) in result_lines.iter().enumerate() {
            if final_view.lines.get(i).is_some() {
                let anchor = visible_anchors[i].clone();
                rendered.push_str(&format!("{}: {}\n", anchor, line));
                structured.push(AnchoredLine {
                    line_number: i + 1,
                    anchor,
                    text: (*line).to_owned(),
                });
            }
        }

        Ok(EditResult {
            path: request.path,
            before_lines,
            content: rendered,
            lines: structured,
            total_lines: result_lines.len(),
        })
    }
}

/// Normalize a path and enforce the configured workspace policy.
fn normalize_path(p: &str, config: &FileToolConfig) -> Result<String> {
    let supplied = Path::new(p);
    let rooted;
    let path = if supplied.is_absolute() {
        supplied
    } else if let Some(root) = config.workspace_root.as_deref() {
        rooted = root.join(supplied);
        rooted.as_path()
    } else {
        supplied
    };
    let normalized = normalize_unchecked(path, config.follow_symlinks)?;

    if !config.allow_outside_workspace {
        let root = config
            .workspace_root
            .as_deref()
            .context("workspace_root is required when outside-workspace access is disabled")?;
        let normalized_root = normalize_unchecked(root, config.follow_symlinks)?;
        if !normalized.starts_with(&normalized_root) {
            bail!(
                "path '{}' is outside configured workspace root '{}'",
                normalized.display(),
                normalized_root.display()
            );
        }
    }

    Ok(normalized.to_string_lossy().into_owned())
}

fn normalize_unchecked(path: &Path, follow_symlinks: bool) -> Result<PathBuf> {
    let absolute = lexical_absolute(path)?;
    if follow_symlinks {
        canonicalize_allow_missing(&absolute)
    } else {
        reject_symlink_components(&absolute)?;
        Ok(absolute)
    }
}

fn lexical_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("failed to make path absolute: {}", path.display()))?;
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                clean.pop();
            }
            std::path::Component::CurDir => {}
            other => clean.push(other),
        }
    }
    Ok(clean)
}

fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .context("path has no existing ancestor")?
            .to_os_string();
        suffix.push(name);
        existing = existing.parent().context("path has no existing ancestor")?;
    }
    let mut canonical = existing
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", existing.display()))?;
    for part in suffix.into_iter().rev() {
        canonical.push(part);
    }
    Ok(canonical)
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("symlink traversal is disabled: {}", current.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manager rooted at a fresh temp directory, plus that directory.
    fn manager_at(name: &str) -> (FileToolManager, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "artist-drift-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        let config = FileToolConfig {
            workspace_root: Some(root.clone()),
            ..FileToolConfig::default()
        };
        (FileToolManager::with_config(config), root)
    }

    async fn read(manager: &mut FileToolManager, path: &str) {
        manager
            .read_file(ReadFileRequest {
                path: path.to_owned(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .expect("read");
    }

    /// The whole point: something outside the harness changed a file the model
    /// had read, and the model finds out without having to fail an edit first.
    #[tokio::test]
    async fn a_file_changed_outside_the_harness_reads_as_drift() {
        let (mut manager, root) = manager_at("outside");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();
        read(&mut manager, &path).await;

        // Stand in for `sed -i`, the user's editor, or another session.
        std::fs::write(&file, "one\nCHANGED\n").expect("outside write");

        let candidates: Vec<_> = manager
            .drift_candidates()
            .into_iter()
            .filter(DriftCandidate::may_have_moved)
            .collect();
        let drifts = manager.describe_drift(&candidates);

        assert_eq!(drifts.len(), 1, "{drifts:?}");
        match &drifts[0].kind {
            DriftKind::Modified {
                before_text,
                after_text,
                ..
            } => {
                assert!(before_text.contains("two"), "{before_text}");
                assert!(after_text.contains("CHANGED"), "{after_text}");
            }
            other => panic!("expected a modification, got {other:?}"),
        }
    }

    /// The common case, and the one that must cost nothing: nothing moved.
    /// A check that reported drift for untouched files would tax every tool
    /// call in the session.
    #[tokio::test]
    async fn an_untouched_file_is_not_drift() {
        let (mut manager, root) = manager_at("untouched");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();
        read(&mut manager, &path).await;

        let candidates: Vec<_> = manager
            .drift_candidates()
            .into_iter()
            .filter(DriftCandidate::may_have_moved)
            .collect();
        assert!(manager.describe_drift(&candidates).is_empty());
    }

    /// The harness's own writes must not report as drift, or every edit would
    /// be followed by a report of itself. Nothing classifies tools as native —
    /// this holds because a write updates the view it is compared against.
    #[tokio::test]
    async fn our_own_write_does_not_report_as_drift() {
        let (mut manager, root) = manager_at("ourwrite");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let path = file.to_string_lossy().into_owned();
        read(&mut manager, &path).await;

        manager
            .write_file(WriteFileRequest {
                path: path.clone(),
                content: "one\ntwo\n".to_owned(),
                overwrite: true,
            })
            .await
            .expect("write");

        let candidates: Vec<_> = manager
            .drift_candidates()
            .into_iter()
            .filter(DriftCandidate::may_have_moved)
            .collect();
        assert!(
            manager.describe_drift(&candidates).is_empty(),
            "a write reported itself as drift"
        );
    }

    /// mtime can move without content moving — a touch, a checkout restoring
    /// identical bytes, a rewrite of the same text. The stat prefilter lets
    /// those through on purpose; the hash is what decides.
    #[tokio::test]
    async fn an_identical_rewrite_is_not_drift() {
        let (mut manager, root) = manager_at("identical");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\ntwo\n").expect("seed");
        let path = file.to_string_lossy().into_owned();
        read(&mut manager, &path).await;

        std::fs::write(&file, "one\ntwo\n").expect("rewrite");

        let candidates = manager.drift_candidates();
        assert!(
            manager.describe_drift(&candidates).is_empty(),
            "same bytes reported as a change"
        );
    }

    /// A vanished file is not a modified one: "re-read it" is the wrong advice.
    #[tokio::test]
    async fn a_deleted_file_reads_as_deleted() {
        let (mut manager, root) = manager_at("deleted");
        let file = root.join("a.txt");
        std::fs::write(&file, "one\n").expect("seed");
        let path = file.to_string_lossy().into_owned();
        read(&mut manager, &path).await;

        std::fs::remove_file(&file).expect("delete");

        let candidates = manager.drift_candidates();
        let drifts = manager.describe_drift(&candidates);
        assert_eq!(drifts.len(), 1, "{drifts:?}");
        assert!(matches!(drifts[0].kind, DriftKind::Deleted));

        // Reported once, then forgotten: a file that stays deleted must not
        // re-report on every subsequent tool call for the rest of the session.
        let candidates = manager.drift_candidates();
        assert!(manager.describe_drift(&candidates).is_empty());
    }

    /// Anchors on surviving lines are the model's existing handles, so a drift
    /// report only has to explain the part that moved.
    #[tokio::test]
    async fn surviving_lines_keep_the_anchors_the_model_holds() {
        let (mut manager, root) = manager_at("survivors");
        let file = root.join("a.txt");
        std::fs::write(&file, "keep\ngoing\n").expect("seed");
        let path = file.to_string_lossy().into_owned();
        read(&mut manager, &path).await;

        let held = manager
            .last_read_view
            .get(&path)
            .map(FileToolManager::render_view)
            .expect("a view");
        let keep_anchor = held[0].anchor.clone();
        assert!(!keep_anchor.is_empty());

        std::fs::write(&file, "keep\nCHANGED\n").expect("outside write");
        let candidates = manager.drift_candidates();
        let drifts = manager.describe_drift(&candidates);

        match &drifts[0].kind {
            DriftKind::Modified { after, .. } => {
                assert_eq!(
                    after[0].anchor, keep_anchor,
                    "an untouched line was reissued a new anchor"
                );
            }
            other => panic!("expected a modification, got {other:?}"),
        }
    }

    #[test]
    fn normalize_insertion_reterminates_and_normalizes_crlf() {
        // A bare-LF-terminated line normalizes to the file's CRLF.
        assert_eq!(normalize_insertion("// note\n", "\r\n"), "// note\r\n");
        // Missing trailing terminator is added.
        assert_eq!(normalize_insertion("a\nb", "\n"), "a\nb\n");
        // Mixed input collapses to the file's terminator.
        assert_eq!(normalize_insertion("a\r\nb\n", "\n"), "a\nb\n");
        // Empty content inserts a single blank line.
        assert_eq!(normalize_insertion("", "\r\n"), "\r\n");
    }

    #[test]
    fn file_view_renders_teca_addresses_for_text_and_rust() {
        for (path, text) in [
            (Path::new("test.txt"), "hello\nworld\nfoo\n"),
            (Path::new("test.rs"), "fn main() {\n    let x = 1;\n}\n"),
        ] {
            let view = FileView::from_text(text, path);
            assert_eq!(view.lines.len(), 3);
            assert!(view.lines.iter().all(|line| line.anchor.starts_with('#')));
            let mut anchors: Vec<&str> =
                view.lines.iter().map(|line| line.anchor.as_str()).collect();
            anchors.sort_unstable();
            anchors.dedup();
            assert_eq!(anchors.len(), 3);
        }
    }

    #[tokio::test]
    async fn edit_anchor_matching_is_exact_and_opaque() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("opaque.txt");
        let p = path.to_string_lossy().into_owned();
        tokio::fs::write(&path, "alpha\nbeta\n").await.unwrap();
        let mut manager = FileToolManager::new();
        let read = manager
            .read_file(ReadFileRequest {
                path: p.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let anchor = read.lines[1].anchor.clone();
        for altered in [
            format!(" {anchor}"),
            format!("{anchor} "),
            anchor.to_uppercase(),
        ] {
            if altered == anchor {
                continue;
            }
            let error = manager
                .preview_edit_file(EditRequest {
                    path: p.clone(),
                    operations: vec![EditOperation::Replace {
                        anchor: altered,
                        end_anchor: None,
                        content: "BETA".into(),
                    }],
                })
                .await
                .unwrap_err();
            assert!(error.to_string().contains("does not exactly resolve"));
        }
    }

    #[tokio::test]
    async fn replace_normalizes_one_trailing_line_terminator() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("replace_newline.txt");
        let p = path.to_str().unwrap().to_string();
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let mut mgr = FileToolManager::new();
        let read = mgr
            .read_file(ReadFileRequest {
                path: p.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let beta = read.lines[1].anchor.clone();

        mgr.edit_file(EditRequest {
            path: p.clone(),
            operations: vec![EditOperation::Replace {
                anchor: beta,
                end_anchor: None,
                content: "BETA\n".to_string(),
            }],
        })
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "alpha\nBETA\ngamma\n");
    }

    #[tokio::test]
    async fn multi_operation_handles_adjacent_insert_replace_delete() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("multi_adjacent.txt");
        let p = path.to_str().unwrap().to_string();
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let mut mgr = FileToolManager::new();
        let read = mgr
            .read_file(ReadFileRequest {
                path: p.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let alpha = read.lines[0].anchor.clone();
        let beta = read.lines[1].anchor.clone();
        let gamma = read.lines[2].anchor.clone();

        mgr.edit_file(EditRequest {
            path: p.clone(),
            operations: vec![
                EditOperation::InsertAfter {
                    anchor: alpha,
                    content: "inserted".to_string(),
                },
                EditOperation::Replace {
                    anchor: beta,
                    end_anchor: None,
                    content: "BETA_EDITED".to_string(),
                },
                EditOperation::Delete {
                    anchor: gamma,
                    end_anchor: None,
                },
            ],
        })
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "alpha\ninserted\nBETA_EDITED\n");
    }

    #[tokio::test]
    async fn insert_inside_delete_range_is_rejected_without_corruption() {
        // Regression (COR-1): an insert whose position lands strictly inside a
        // Delete/Replace range used to slip past validation (an insert is an
        // empty byte range) and then corrupt the file when the range op drained
        // stale offsets. It must now be rejected, leaving the file untouched.
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("insert_in_range.txt");
        let p = path.to_str().unwrap().to_string();
        let original = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        tokio::fs::write(&path, original).await.unwrap();

        let mut mgr = FileToolManager::new();
        let read = mgr
            .read_file(ReadFileRequest {
                path: p.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let alpha = read.lines[0].anchor.clone();
        let gamma = read.lines[2].anchor.clone();
        let epsilon = read.lines[4].anchor.clone();

        let result = mgr
            .edit_file(EditRequest {
                path: p.clone(),
                operations: vec![
                    EditOperation::Delete {
                        anchor: alpha,
                        end_anchor: Some(epsilon),
                    },
                    EditOperation::InsertBefore {
                        anchor: gamma,
                        content: "intruder".to_string(),
                    },
                ],
            })
            .await;

        assert!(result.is_err(), "interior insert should be rejected");
        let after = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(after, original, "file must be left untouched on rejection");
    }

    #[tokio::test]
    async fn equal_position_insertions_preserve_request_order() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("same_offset_insertions.txt");
        let p = path.to_str().unwrap().to_string();
        tokio::fs::write(&path, "alpha\nbeta\n").await.unwrap();

        let mut mgr = FileToolManager::new();
        let read = mgr
            .read_file(ReadFileRequest {
                path: p.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let alpha = read.lines[0].anchor.clone();
        let beta = read.lines[1].anchor.clone();

        mgr.edit_file(EditRequest {
            path: p.clone(),
            operations: vec![
                EditOperation::InsertAfter {
                    anchor: alpha,
                    content: "after-alpha".to_string(),
                },
                EditOperation::InsertBefore {
                    anchor: beta,
                    content: "before-beta".to_string(),
                },
            ],
        })
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "alpha\nafter-alpha\nbefore-beta\nbeta\n");
    }
    #[tokio::test]
    async fn moved_occurrence_resolves_without_relocation_state() {
        // Moving a unique occurrence changes its position but not its semantic identity.
        // The exact old anchor therefore resolves without relocation state or confirmation.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("reloc.txt").to_str().unwrap().to_string();
        tokio::fs::write(&p, "first line\nsecond line\nthird line\n")
            .await
            .unwrap();

        let mut mgr = FileToolManager::new();
        let read = mgr
            .read_file(ReadFileRequest {
                path: p.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let first_prefix = read
            .content
            .lines()
            .next()
            .unwrap()
            .split_once(": ")
            .unwrap()
            .0
            .to_string();

        // Reorder so "first line" moves to position 3
        tokio::fs::write(&p, "second line\nthird line\nfirst line\n")
            .await
            .unwrap();

        let result = mgr
            .edit_file(EditRequest {
                path: p.clone(),
                operations: vec![EditOperation::Replace {
                    end_anchor: None,
                    anchor: first_prefix.clone(),
                    content: "replaced moved".to_string(),
                }],
            })
            .await
            .unwrap();
        assert!(result.content.contains("replaced moved"));
        let fc = tokio::fs::read_to_string(&p).await.unwrap();
        assert_eq!(fc, "second line\nthird line\nreplaced moved\n");
    }

    // -----------------------------------------------------------------------
    // Path independence: path scopes the operation but is not identity input
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn path_is_not_part_of_occurrence_identity() {
        let tmpdir = tempfile::tempdir().unwrap();
        let p1 = tmpdir.path().join("file_a.txt");
        let p2 = tmpdir.path().join("file_b.txt");
        tokio::fs::write(&p1, "alpha\nbeta\n").await.unwrap();
        tokio::fs::write(&p2, "alpha\nbeta\n").await.unwrap();
        let s1 = p1.to_str().unwrap().to_string();
        let s2 = p2.to_str().unwrap().to_string();

        let mut mgr = FileToolManager::new();
        let r1 = mgr
            .read_file(ReadFileRequest {
                path: s1.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let r2 = mgr
            .read_file(ReadFileRequest {
                path: s2.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();

        assert_eq!(r1.lines[0].anchor, r2.lines[0].anchor);
        assert_eq!(r1.lines[1].anchor, r2.lines[1].anchor);

        mgr.edit_file(EditRequest {
            path: s2,
            operations: vec![EditOperation::Replace {
                end_anchor: None,
                anchor: r1.lines[0].anchor.clone(),
                content: "replaced alpha".to_string(),
            }],
        })
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read_to_string(&p1).await.unwrap(),
            "alpha\nbeta\n",
            "the requested file path scopes resolution but is not identity input"
        );
        assert_eq!(
            tokio::fs::read_to_string(&p2).await.unwrap(),
            "replaced alpha\nbeta\n"
        );
    }

    // -----------------------------------------------------------------------
    // Multi-operation no-borrow test (Requirement 2)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Atomic failure test (Requirement 3)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_atomic_failure_leaves_file_unchanged() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("atomic.txt");
        let p = path.to_str().unwrap().to_string();
        tokio::fs::write(&path, "keep\nkeep\nkeep\nkeep\n")
            .await
            .unwrap();

        let mut mgr = FileToolManager::new();
        mgr.read_file(ReadFileRequest {
            path: p.clone(),
            start_line: 1,
            max_lines: None,
        })
        .await
        .unwrap();

        // An edit referencing an anchor that was never issued must fail before
        // any write touches disk.
        let err = mgr
            .edit_file(EditRequest {
                path: p.clone(),
                operations: vec![EditOperation::Replace {
                    end_anchor: None,
                    anchor: "nonexistent".to_string(),
                    content: "should NOT write".to_string(),
                }],
            })
            .await
            .unwrap_err();
        assert!(!err.to_string().is_empty(), "edit must fail before writing");

        // File must be UNCHANGED (atomicity).
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            content, "keep\nkeep\nkeep\nkeep\n",
            "file must not be modified after a failed atomic batch"
        );
    }

    // -----------------------------------------------------------------------
    // Determinism and equivalent-occurrence symmetry breaking
    // -----------------------------------------------------------------------

    #[test]
    fn repeated_duplicate_blocks_have_deterministic_addresses() {
        let text = "# Identical Blocks Test

## Block
same payload
same payload

## Block
same payload
same payload

## Block
same payload
same payload

End blocks test.
";
        let path = Path::new("test.md");
        let expected: Vec<String> = FileView::from_text(text, path)
            .lines
            .into_iter()
            .map(|line| line.anchor)
            .collect();

        for _ in 0..256 {
            let actual: Vec<String> = FileView::from_text(text, path)
                .lines
                .into_iter()
                .map(|line| line.anchor)
                .collect();
            assert_eq!(
                actual, expected,
                "same file must always produce the same anchors"
            );
        }
    }

    #[test]
    fn equivalent_occurrences_are_unique_and_deterministic() {
        let path = Path::new("test.txt");
        let first = FileView::from_text("A\nA\nA\n", path);
        let second = FileView::from_text("A\nA\nA\n", path);
        let first_anchors: Vec<&str> = first
            .lines
            .iter()
            .map(|line| line.anchor.as_str())
            .collect();
        let second_anchors: Vec<&str> = second
            .lines
            .iter()
            .map(|line| line.anchor.as_str())
            .collect();
        assert_eq!(first_anchors, second_anchors);
        let mut unique = first_anchors.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 3);

        let four = FileView::from_text("A\nA\nA\nA\n", path);
        let mut four_unique: Vec<&str> =
            four.lines.iter().map(|line| line.anchor.as_str()).collect();
        four_unique.sort_unstable();
        four_unique.dedup();
        assert_eq!(four_unique.len(), 4);
    }
}
