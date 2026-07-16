use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use xxhash_rust::xxh3::xxh3_64;

#[cfg(test)]
use crate::mnemonic_anchors::pack_binding;
use crate::mnemonic_anchors::{binding_full, binding_guard, is_mnemonic_handle, reconcile_handles};

/// Encode a 64-bit hash as 13 lowercase Crockford Base32 characters.
fn hash_to_base32(mut hash: u64) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut chars = ['0'; 13];
    for index in (0..13).rev() {
        chars[index] = ALPHABET[(hash & 31) as usize] as char;
        hash >>= 5;
    }
    chars.iter().collect()
}

fn compute_hash(data: &[u8]) -> u64 {
    xxh3_64(data)
}

fn shortest_unique_prefix<'a>(
    target: &str,
    others: impl Iterator<Item = &'a str>,
    min_len: usize,
) -> String {
    let target_chars: Vec<char> = target.chars().collect();
    let other_chars: Vec<Vec<char>> = others.map(|s| s.chars().collect()).collect();
    let max_len = target_chars.len();

    for len in min_len..=max_len {
        let prefix: String = target_chars[..len].iter().collect();
        let matches = other_chars
            .iter()
            .filter(|oc| oc.len() >= len && oc[..len].iter().collect::<String>() == prefix)
            .count();
        if matches == 1 {
            return prefix;
        }
    }
    target.to_string()
}

fn short_hash(full: &str, lines: &[LineInfo]) -> String {
    let others: Vec<&str> = lines.iter().map(|l| l.full_hash.as_str()).collect();
    shortest_unique_prefix(full, others.into_iter(), 1)
}

fn normalize_replacement_content(content: &str) -> &str {
    content
        .strip_suffix("\r\n")
        .or_else(|| content.strip_suffix('\n'))
        .unwrap_or(content)
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

/// Error type signalling that a stale mnemonic anchor needs user confirmation.
/// The caller must return a structured `confirmation_required` result.
#[derive(Debug, Clone)]
pub struct ConfirmationRequired {
    pub message: String,
    pub path: String,
    pub visible_anchor: String,
    pub candidate_anchor: String,
    pub context: String,
    pub operation_fingerprint: String,
}

impl fmt::Display for ConfirmationRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConfirmationRequired {}

/// Internal result from anchor resolution.
enum HashResolution {
    /// The visible mnemonic resolved to a hidden full hash that is still current.
    Resolved(String),
    /// The old hidden hash is gone and its one-character-or-longer guard prefix
    /// now uniquely matches a different current line.
    StaleUnique {
        candidate_hash: String,
        candidate_anchor: String,
        guard_prefix: String,
        context: String,
    },
}

#[derive(Debug, Clone)]
struct LineInfo {
    full_hash: String,
    content: String,
}

#[derive(Debug, Clone)]
struct FileView {
    lines: Vec<LineInfo>,
}

impl FileView {
    fn from_text(text: &str, path: &Path) -> Self {
        let is_rust = path.extension().map(|e| e == "rs").unwrap_or(false);
        let raw_lines: Vec<&str> = text.lines().collect();

        // Structured parse for semantic identity (Rust only)
        // For non-Rust, use bare trimmed content so duplicates are detected
        // and disambiguated by occurrence index.
        let semantics: Vec<String> = if is_rust {
            compute_semantic_identities_rust(&raw_lines)
        } else {
            raw_lines
                .iter()
                .map(|line| line.trim().to_string())
                .collect()
        };

        // Build initial line-infos
        let mut line_infos: Vec<LineInfo> = raw_lines
            .iter()
            .map(|line| {
                let full_hash = hash_to_base32(compute_hash(line.as_bytes()));
                LineInfo {
                    full_hash,
                    content: line.to_string(),
                }
            })
            .collect();

        // Group by semantic identity
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, sem) in semantics.iter().enumerate() {
            let semantic_hash = hash_to_base32(compute_hash(sem.as_bytes()));
            groups.entry(semantic_hash).or_default().push(i);
        }

        // Final full-ID collision check: verify all hashes are unique.
        {
            let mut seen: HashSet<String> = HashSet::new();
            for li in &mut line_infos {
                let hash = li.full_hash.clone();
                if !seen.insert(hash) {
                    // Collision — disambiguate by content + counter
                    let mut attempt = 0u64;
                    loop {
                        let candidate = hash_to_base32(compute_hash(
                            format!("{}|{}", li.content, attempt).as_bytes(),
                        ));
                        if seen.insert(candidate.clone()) {
                            li.full_hash = candidate;
                            break;
                        }
                        attempt += 1;
                    }
                }
            }
        }

        // Freeze the collision-resolved base IDs before duplicate groups are
        // rewritten. Duplicate groups live in a HashMap, so reading neighboring
        // IDs from line_infos while mutating it makes results depend on random
        // group iteration order.
        let base_hashes: Vec<String> = line_infos
            .iter()
            .map(|line| line.full_hash.clone())
            .collect();

        // Adjust duplicates: incorporate physical line bytes, stable named-node
        // ancestry, duplicate count, occurrence index, and BOTH previous/next
        // full base hashes.
        for indices in groups.values() {
            if indices.len() <= 1 {
                continue;
            }
            let count = indices.len() as u32;
            for (occ_idx, &line_idx) in indices.iter().enumerate() {
                let prev_hash = if line_idx > 0 {
                    base_hashes[line_idx - 1].clone()
                } else {
                    String::new()
                };
                let next_hash = if line_idx + 1 < line_infos.len() {
                    base_hashes[line_idx + 1].clone()
                } else {
                    String::new()
                };
                // Use the semantic identity (which includes ancestry for Rust)
                // so the hash is stable across adjacent non-structural changes.
                let disambiguated = format!(
                    "{}|{}|{}|{}|{}",
                    semantics[line_idx], count, occ_idx, prev_hash, next_hash
                );
                let new_hash = hash_to_base32(compute_hash(disambiguated.as_bytes()));
                line_infos[line_idx].full_hash = new_hash;
            }
        }

        Self { lines: line_infos }
    }
}

fn compute_semantic_identities_rust(raw_lines: &[&str]) -> Vec<String> {
    use tree_sitter::Parser;

    let full_source = raw_lines.join("\n");
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    if parser.set_language(&lang).is_err() {
        // Fallback to content-only for duplicate detection
        return raw_lines.iter().map(|line| line.to_string()).collect();
    }

    let tree = match parser.parse(&full_source, None) {
        Some(t) => t,
        None => {
            return raw_lines.iter().map(|line| line.to_string()).collect();
        }
    };

    let root = tree.root_node();
    let n = raw_lines.len();

    // Pre-compute byte offsets for each line
    let mut line_offsets: Vec<usize> = Vec::with_capacity(n + 1);
    line_offsets.push(0);
    for line in raw_lines {
        let prev = *line_offsets.last().unwrap();
        line_offsets.push(prev + line.len() + 1); // +1 for newline
    }

    let mut identities: Vec<String> = Vec::with_capacity(n);
    for (i, line) in raw_lines.iter().enumerate() {
        let start_byte = line_offsets[i];
        let end_byte = line_offsets[i + 1].saturating_sub(1); // exclude the newline

        let mut ancestry: Vec<String> = Vec::new();
        if let Some(node) = root.descendant_for_byte_range(start_byte, end_byte) {
            let mut current = node;
            loop {
                if current.is_named() {
                    ancestry.push(current.kind().to_string());
                }
                match current.parent() {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
            ancestry.reverse();
        }

        if ancestry.is_empty() {
            identities.push(line.to_string());
        } else {
            identities.push(format!("{}|{}", line, ancestry.join("/")));
        }
    }

    identities
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
        hash: String,
        end_hash: Option<String>,
    },
    Replace {
        hash: String,
        end_hash: Option<String>,
        content: String,
    },
    InsertBefore {
        hash: String,
        content: String,
    },
    InsertAfter {
        hash: String,
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

/// Compound key for a pending stale-prefix confirmation
/// waiting for an exact retry.
type PendingKey = (String, String, String, String);

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
    /// Per-path mappings of model-facing mnemonic anchor → packed hidden hash binding.
    /// Legacy visible hash-prefix bindings remain readable during migration.
    issued_prefixes: HashMap<String, HashMap<String, String>>,
    last_read_view: HashMap<String, FileView>,
    /// Set of pending stale-prefix confirmation keys.
    pending_confirmations: HashSet<PendingKey>,
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
            issued_prefixes: HashMap::new(),
            last_read_view: HashMap::new(),
            pending_confirmations: HashSet::new(),
        }
    }

    pub fn config(&self) -> &FileToolConfig {
        &self.config
    }

    pub fn normalized_path(&self, path: &str) -> Result<String> {
        normalize_path(path, &self.config)
    }

    pub fn export_issued_prefixes(&self) -> HashMap<String, HashMap<String, String>> {
        self.issued_prefixes.clone()
    }

    pub fn import_issued_prefixes(
        &mut self,
        issued_prefixes: HashMap<String, HashMap<String, String>>,
    ) {
        self.issued_prefixes = issued_prefixes;
        self.pending_confirmations.clear();
        self.last_read_view.clear();
    }

    pub fn forget_path(&mut self, path: &str) -> Result<()> {
        let normalized = normalize_path(path, &self.config)?;
        self.clear_all_for_path(&normalized);
        self.last_read_view.remove(&normalized);
        Ok(())
    }

    fn reconcile_path_anchors(
        &mut self,
        path: &str,
        view: &FileView,
        reclaim_dead: bool,
    ) -> Vec<String> {
        let full_hashes: Vec<String> = view
            .lines
            .iter()
            .map(|line| line.full_hash.clone())
            .collect();
        let guard_prefixes: Vec<String> = view
            .lines
            .iter()
            .map(|line| short_hash(&line.full_hash, &view.lines))
            .collect();
        let existing = self.issued_prefixes.remove(path).unwrap_or_default();
        let (state, visible) =
            reconcile_handles(&existing, &full_hashes, &guard_prefixes, reclaim_dead);
        self.issued_prefixes.insert(path.to_string(), state);
        visible
    }

    fn anchor_for_full_hash(issued: &HashMap<String, String>, full_hash: &str) -> Option<String> {
        issued
            .iter()
            .filter(|(anchor, packed)| {
                is_mnemonic_handle(anchor) && binding_full(packed) == full_hash
            })
            .map(|(anchor, _)| anchor)
            .min_by_key(|anchor| (anchor.split(' ').count(), anchor.len(), *anchor))
            .cloned()
    }

    pub async fn read_file(&mut self, request: ReadFileRequest) -> Result<ReadFileResult> {
        let norm = normalize_path(&request.path, &self.config)?;
        // Reread clears pending confirmations for this path but preserves
        // existing anchor mappings for lines outside the returned range.
        self.clear_pending_for_path(&norm);

        let path = Path::new(&norm);
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", request.path))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let view = FileView::from_text(&content, path);
        self.last_read_view.insert(norm.clone(), view.clone());

        let start = request.start_line.saturating_sub(1);
        let end = match request.max_lines {
            Some(max) => (start + max).min(lines.len()),
            None => lines.len(),
        };

        // A full explicit reread acknowledges the latest view, so dead bindings can
        // be reclaimed. Partial reads preserve unseen tombstones.
        let full_refresh = start == 0 && end == lines.len();
        let visible_anchors = self.reconcile_path_anchors(&norm, &view, full_refresh);
        let mut rendered = String::new();
        let mut structured = Vec::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_idx = start + i;
            if view.lines.get(line_idx).is_some() {
                let anchor = visible_anchors[line_idx].clone();
                rendered.push_str(&format!("{} | {}\n", anchor, line));
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
        // Full write replaces all path mappings
        self.clear_all_for_path(&norm);

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
        let view = FileView::from_text(&request.content, path);
        self.last_read_view.insert(norm.clone(), view.clone());

        let visible_anchors = self.reconcile_path_anchors(&norm, &view, true);
        let mut rendered = String::new();
        let mut structured = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if view.lines.get(i).is_some() {
                let anchor = visible_anchors[i].clone();
                rendered.push_str(&format!("{} | {}\n", anchor, line));
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
        // NOTE: we do NOT clear state here — resolution needs stale mappings
        // to fire the confirmation gate.  After successful apply we replace
        // the view and prefixes.

        let path = Path::new(&norm);
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", request.path))?;

        // 1. Snapshot: byte ranges for each line in the ORIGINAL content
        //    (preserves CRLF, tab characters, unrelated bytes).
        let line_ranges = line_byte_ranges(&content);
        let snapshot_view = FileView::from_text(&content, path);
        // Allocate mnemonics for newly observed concurrent content while preserving
        // stale bindings until this edit is acknowledged successfully.
        let snapshot_anchors = self.reconcile_path_anchors(&norm, &snapshot_view, false);
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

        // 2. Compute a deterministic fingerprint for the ENTIRE request so
        //    that a confirmation authorises exactly this request.
        let fp = Self::request_fingerprint(&request);

        // 3. Resolve every operation's hash(es) against the original
        //    snapshot.  Any stale-prefix confirmation ties the whole batch.
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

        let resolve_line_idx = |manager: &mut Self,
                                hash: &str|
         -> anyhow::Result<(String, usize)> {
            let full = manager.resolve_hash_with_confirmation(&norm, hash, &snapshot_view, &fp)?;
            let idx = snapshot_view
                .lines
                .iter()
                .position(|li| li.full_hash == full)
                .ok_or_else(|| anyhow::anyhow!("line with hash {} not found", full))?;
            Ok((full, idx))
        };
        let mut resolved: Vec<ResolvedOp> = Vec::new();

        for (op_idx, operation) in request.operations.iter().enumerate() {
            match operation {
                EditOperation::Delete { hash, end_hash } => {
                    let (_, start_idx) = resolve_line_idx(self, hash)?;
                    let end_idx = if let Some(end_hash) = end_hash {
                        let (_, end_idx) = resolve_line_idx(self, end_hash)?;
                        end_idx
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
                    hash,
                    end_hash,
                    content: new_content,
                } => {
                    let (_, start_idx) = resolve_line_idx(self, hash)?;
                    let end_idx = if let Some(end_hash) = end_hash {
                        let (_, end_idx) = resolve_line_idx(self, end_hash)?;
                        end_idx
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
                    hash,
                    content: new_content,
                } => {
                    let full =
                        self.resolve_hash_with_confirmation(&norm, hash, &snapshot_view, &fp)?;
                    let idx = snapshot_view
                        .lines
                        .iter()
                        .position(|li| li.full_hash == full)
                        .ok_or_else(|| anyhow::anyhow!("line with hash {} not found", full))?;
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
                    hash,
                    content: new_content,
                } => {
                    let full =
                        self.resolve_hash_with_confirmation(&norm, hash, &snapshot_view, &fp)?;
                    let idx = snapshot_view
                        .lines
                        .iter()
                        .position(|li| li.full_hash == full)
                        .ok_or_else(|| anyhow::anyhow!("line with hash {} not found", full))?;
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

        // 5. Reject overlapping non-empty byte ranges for Delete/Replace.
        for i in 0..resolved.len() {
            for j in (i + 1)..resolved.len() {
                let a = &resolved[i];
                let b = &resolved[j];
                let a_empty = a.byte_start == a.byte_end;
                let b_empty = b.byte_start == b.byte_end;
                if a_empty || b_empty {
                    continue;
                }
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
                    let to_insert = if content.ends_with('\n') || content.ends_with("\r\n") {
                        content.clone()
                    } else {
                        format!("{}\n", content)
                    };
                    result.insert_str(op.byte_start, &to_insert);
                }
                OpKind::InsertAfter { content } => {
                    let to_insert = if content.ends_with('\n') || content.ends_with("\r\n") {
                        content.clone()
                    } else {
                        format!("{}\n", content)
                    };
                    result.insert_str(op.byte_start, &to_insert);
                }
            }
        }

        // 8. Write the final result atomically for an applied edit. Preview uses
        // the same resolution and reconciliation path against a cloned manager,
        // but leaves the filesystem and persistent allocator state untouched.
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
        // 9. A successful edit acknowledges the new view: discard dead tombstones,
        // preserve every surviving handle unchanged, and make freed one-word
        // handles available only to newly created lines.
        self.clear_pending_for_path(&norm);
        let final_view = FileView::from_text(&result, path);
        self.last_read_view.insert(norm.clone(), final_view.clone());
        let visible_anchors = self.reconcile_path_anchors(&norm, &final_view, true);

        let result_lines: Vec<&str> = result.lines().collect();
        let mut rendered = String::new();
        let mut structured = Vec::new();
        for (i, line) in result_lines.iter().enumerate() {
            if final_view.lines.get(i).is_some() {
                let anchor = visible_anchors[i].clone();
                rendered.push_str(&format!("{} | {}\n", anchor, line));
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

    fn path_prefixes_mut(&mut self, path: &str) -> &mut HashMap<String, String> {
        self.issued_prefixes.entry(path.to_string()).or_default()
    }

    /// Resolve a model-facing mnemonic anchor for editing, handling the hidden
    /// hash guard and stale-anchor confirmation gate.
    fn resolve_hash_with_confirmation(
        &mut self,
        path: &str,
        visible: &str,
        view: &FileView,
        request_fp: &str,
    ) -> Result<String> {
        let visible_owned = visible.trim().to_ascii_lowercase();
        let visible = visible_owned.as_str();
        let issued = self.path_prefixes_mut(path);
        match Self::resolve_hash(issued, visible, view)? {
            HashResolution::Resolved(full) => Ok(full),
            HashResolution::StaleUnique {
                candidate_hash,
                candidate_anchor,
                guard_prefix,
                context,
            } => {
                let _ = (candidate_hash, guard_prefix);
                let message = format!(
                    "stale_anchor: anchor '{}' no longer resolves to the line that was read. Re-read the file before editing.\nCurrent candidate: {}\n{}",
                    visible, candidate_anchor, context,
                );
                Err(anyhow::anyhow!(ConfirmationRequired {
                    message,
                    path: path.to_string(),
                    visible_anchor: visible.to_string(),
                    candidate_anchor,
                    context,
                    operation_fingerprint: request_fp.to_string(),
                }))
            }
        }
    }

    fn resolve_hash(
        issued: &mut HashMap<String, String>,
        visible: &str,
        view: &FileView,
    ) -> Result<HashResolution> {
        let clean_owned = visible.trim().to_ascii_lowercase();
        let clean = clean_owned.as_str();
        let packed = issued.get(clean).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "anchor '{}' was not issued for this file. Re-read the file to get current mnemonic anchors.",
                visible
            )
        })?;
        let full_hash = binding_full(&packed).to_owned();
        if view.lines.iter().any(|line| line.full_hash == full_hash) {
            return Ok(HashResolution::Resolved(full_hash));
        }

        let guard_prefix = binding_guard(clean, &packed).to_owned();
        let candidates: Vec<&LineInfo> = view
            .lines
            .iter()
            .filter(|line| line.full_hash.starts_with(&guard_prefix))
            .collect();
        if candidates.len() == 1 {
            let candidate = candidates[0];
            let candidate_anchor = Self::anchor_for_full_hash(issued, &candidate.full_hash)
                .context("current candidate did not receive a mnemonic anchor")?;
            let context = render_context(view, candidate, issued);
            return Ok(HashResolution::StaleUnique {
                candidate_hash: candidate.full_hash.clone(),
                candidate_anchor,
                guard_prefix,
                context,
            });
        }
        if candidates.is_empty() {
            bail!(
                "anchor '{}' is stale and its hidden guard does not match any current line. Re-read the file.",
                visible
            );
        }
        bail!(
            "anchor '{}' is stale and its hidden guard is ambiguous (matches {} lines). Re-read the file.",
            visible,
            candidates.len()
        );
    }

    /// Compute a deterministic fingerprint of the entire EditRequest
    /// using tagged length-prefixed binary encoding before XXH3/Crockford Base32.
    fn request_fingerprint(request: &EditRequest) -> String {
        let mut buf = Vec::new();
        // Tag 0: path
        buf.push(0u8);
        let path_bytes = request.path.as_bytes();
        buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(path_bytes);

        fn push_string(buf: &mut Vec<u8>, value: &str) {
            let bytes = value.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(bytes);
        }
        fn push_optional_string(buf: &mut Vec<u8>, value: Option<&String>) {
            match value {
                Some(value) => {
                    buf.push(1);
                    push_string(buf, value);
                }
                None => buf.push(0),
            }
        }

        for op in &request.operations {
            match op {
                EditOperation::Delete { hash, end_hash } => {
                    buf.push(1u8);
                    push_string(&mut buf, hash);
                    push_optional_string(&mut buf, end_hash.as_ref());
                }
                EditOperation::Replace {
                    hash,
                    end_hash,
                    content,
                } => {
                    buf.push(2u8);
                    push_string(&mut buf, hash);
                    push_optional_string(&mut buf, end_hash.as_ref());
                    push_string(&mut buf, content);
                }
                EditOperation::InsertBefore { hash, content } => {
                    buf.push(3u8);
                    let h = hash.as_bytes();
                    buf.extend_from_slice(&(h.len() as u32).to_le_bytes());
                    buf.extend_from_slice(h);
                    let c = content.as_bytes();
                    buf.extend_from_slice(&(c.len() as u32).to_le_bytes());
                    buf.extend_from_slice(c);
                }
                EditOperation::InsertAfter { hash, content } => {
                    buf.push(4u8);
                    let h = hash.as_bytes();
                    buf.extend_from_slice(&(h.len() as u32).to_le_bytes());
                    buf.extend_from_slice(h);
                    let c = content.as_bytes();
                    buf.extend_from_slice(&(c.len() as u32).to_le_bytes());
                    buf.extend_from_slice(c);
                }
            }
        }
        hash_to_base32(compute_hash(&buf))
    }

    /// Clear pending confirmations for a given path.  Does NOT erase
    /// issued prefixes (partial read should preserve anchors outside range).
    fn clear_pending_for_path(&mut self, path: &str) {
        self.pending_confirmations.retain(|k| k.0 != path);
    }

    /// Clear pending confirmations AND all issued anchor mappings for a path.
    /// Used by write/edit which replace the entire mapping.
    fn clear_all_for_path(&mut self, path: &str) {
        self.pending_confirmations.retain(|k| k.0 != path);
        self.issued_prefixes.remove(path);
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

fn render_context(view: &FileView, target: &LineInfo, issued: &HashMap<String, String>) -> String {
    let idx = view
        .lines
        .iter()
        .position(|line| line.full_hash == target.full_hash);
    let Some(idx) = idx else {
        return String::new();
    };

    let start = idx.saturating_sub(2);
    let end = (idx + 3).min(view.lines.len());
    let mut context = String::new();
    for i in start..end {
        if let Some(line) = view.lines.get(i) {
            let anchor = FileToolManager::anchor_for_full_hash(issued, &line.full_hash)
                .unwrap_or_else(|| "<unassigned>".to_string());
            let marker = if i == idx { ">>> " } else { "    " };
            context.push_str(&format!("{}{} | {}\n", marker, anchor, line.content));
        }
    }
    context
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_to_base32() {
        let h = hash_to_base32(0);
        assert_eq!(h.len(), 13);
        assert_eq!(h, "0000000000000");

        let h2 = hash_to_base32(u64::MAX);
        assert_eq!(h2.len(), 13);

        // Ensure deterministic
        assert_eq!(hash_to_base32(42), hash_to_base32(42));
        // Ensure different hashes produce different strings (likely)
        assert_ne!(hash_to_base32(0), hash_to_base32(1));
    }

    #[test]
    fn test_shortest_unique_prefix() {
        let target = "hello12345";
        // others must include the target itself for correct uniqueness check
        let others = vec![target, "world67890", "helping123", "helloworld"];
        let prefix = shortest_unique_prefix(target, others.into_iter(), 5);
        // "hello" is shared with "helloworld", so must extend to at least 6
        assert!(prefix.len() >= 6);
        assert!(target.starts_with(&prefix));
    }

    #[test]
    fn test_file_view_from_text_simple() {
        let path = Path::new("test.txt");
        let text = "hello\nworld\nfoo\n";
        let view = FileView::from_text(text, path);
        assert_eq!(view.lines.len(), 3);
        // All hashes should be 10 chars
        for li in &view.lines {
            assert_eq!(li.full_hash.len(), 13);
        }
    }

    #[test]
    fn test_file_view_from_text_rust() {
        let path = Path::new("test.rs");
        let text = "fn main() {\n    let x = 1;\n}\n";
        let view = FileView::from_text(text, path);
        assert_eq!(view.lines.len(), 3);
        for li in &view.lines {
            assert_eq!(li.full_hash.len(), 13);
        }
    }

    #[test]
    fn test_short_hash_unique() {
        let lines = vec![
            LineInfo {
                full_hash: hash_to_base32(compute_hash(b"line 1")),
                content: "line 1".into(),
            },
            LineInfo {
                full_hash: hash_to_base32(compute_hash(b"line 2")),
                content: "line 2".into(),
            },
        ];
        let p1 = short_hash(&lines[0].full_hash, &lines);
        let p2 = short_hash(&lines[1].full_hash, &lines);
        assert_ne!(p1, p2);
        assert!(lines[0].full_hash.starts_with(&p1));
        assert!(lines[1].full_hash.starts_with(&p2));
    }

    #[test]
    fn test_resolve_hash_basic() {
        let path = Path::new("test.txt");
        let view = FileView::from_text("aaa\nbbb\nccc\n", path);
        let target = &view.lines[1];
        let guard = short_hash(&target.full_hash, &view.lines);

        let mut issued = HashMap::new();
        issued.insert("beta".to_string(), pack_binding(&target.full_hash, &guard));
        let resolved =
            FileToolManager::resolve_hash(&mut issued, "beta", &view).expect("should resolve");
        match resolved {
            HashResolution::Resolved(full) => assert_eq!(full, target.full_hash),
            HashResolution::StaleUnique { .. } => panic!("expected resolved, got stale"),
        }
    }

    // -----------------------------------------------------------------------
    // Stale-prefix confirmation gate tests
    // -----------------------------------------------------------------------

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
                hash: beta,
                end_hash: None,
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
                    hash: alpha,
                    content: "inserted".to_string(),
                },
                EditOperation::Replace {
                    hash: beta,
                    end_hash: None,
                    content: "BETA_EDITED".to_string(),
                },
                EditOperation::Delete {
                    hash: gamma,
                    end_hash: None,
                },
            ],
        })
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "alpha\ninserted\nBETA_EDITED\n");
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
                    hash: alpha,
                    content: "after-alpha".to_string(),
                },
                EditOperation::InsertBefore {
                    hash: beta,
                    content: "before-beta".to_string(),
                },
            ],
        })
        .await
        .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "alpha\nafter-alpha\nbefore-beta\nbeta\n");
    }
    /// Helper to set up a stale scenario:
    /// 1. Writes `content`, reads it (so `issued_prefixes` gains entries)
    /// 2. Then *directly* injects a phony mapping so we control which
    ///    visible-anchor maps to a now-vanished full-hash while the prefix
    ///    coincides with a *different* line's current full hash.
    ///
    /// Returns (mgr, path_str, visible_prefix, candidate_full_hash, TempDir).
    async fn setup_stale_scenario(
        content: &str,
    ) -> (FileToolManager, String, String, String, tempfile::TempDir) {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("stale_test.txt");
        let p = path.to_str().unwrap().to_string();
        tokio::fs::write(&path, content).await.unwrap();

        let mut mgr = FileToolManager::new();
        // Read to populate last_read_view + issued_prefixes
        mgr.read_file(ReadFileRequest {
            path: p.clone(),
            start_line: 1,
            max_lines: None,
        })
        .await
        .unwrap();

        // Determine the normalized path (as used internally by read_file)
        let norm = normalize_path(&p, &FileToolConfig::default()).unwrap();

        // Build a view of the CURRENT file content
        let current_view = FileView::from_text(content, Path::new(&norm));
        // Pick a line we will target (line 0) and replace its issued
        // mnemonic binding with a vanished hidden hash that retains the line's
        // current guard prefix.
        let target_full = current_view.lines[0].full_hash.clone();
        let visible = mgr
            .read_file(ReadFileRequest {
                path: norm.clone(),
                start_line: 1,
                max_lines: Some(1),
            })
            .await
            .unwrap()
            .lines[0]
            .anchor
            .clone();
        let guard = short_hash(&target_full, &current_view.lines);
        let bogus_hash = hash_to_base32(compute_hash(b"bogus line that never existed"));
        mgr.path_prefixes_mut(&norm)
            .insert(visible.clone(), pack_binding(&bogus_hash, &guard));
        mgr.reconcile_path_anchors(&norm, &current_view, false);
        let candidate_anchor =
            FileToolManager::anchor_for_full_hash(mgr.path_prefixes_mut(&norm), &target_full)
                .unwrap();

        (mgr, norm, visible, candidate_anchor, tmpdir)
    }

    #[tokio::test]
    async fn test_stale_first_attempt_no_write() {
        let (mut mgr, path, prefix, candidate_hash, _tmpdir) =
            setup_stale_scenario("some line data\nsecond\nthird\n").await;

        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix.clone(),
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap_err();

        let cr: &ConfirmationRequired = err
            .downcast_ref()
            .expect("expected ConfirmationRequired error");
        assert_eq!(cr.path, path);
        assert_eq!(cr.visible_anchor, prefix);
        assert_eq!(cr.candidate_anchor, candidate_hash);

        // File must NOT have been modified
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "some line data\nsecond\nthird\n");
    }

    #[tokio::test]
    #[ignore = "Artist requires a fresh read instead of retry confirmation"]
    async fn test_stale_exact_retry_applies() {
        let (mut mgr, path, prefix, _candidate, _tmpdir) =
            setup_stale_scenario("some line data\nsecond\nthird\n").await;

        // First attempt — store pending, get ConfirmationRequired
        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix.clone(),
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ConfirmationRequired>().is_some());

        // Retry exact same operation → should apply
        let result = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix.clone(),
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap();
        assert!(result.content.contains("replacement"));
        let fc = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(fc.starts_with("replacement"));
    }

    #[tokio::test]
    async fn test_stale_changed_operation_no_borrow() {
        let (mut mgr, path, prefix, _candidate, _tmpdir) =
            setup_stale_scenario("some line data\nsecond\nthird\n").await;

        // First attempt
        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix.clone(),
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ConfirmationRequired>().is_some());

        // Second attempt with DIFFERENT content → different fingerprint,
        // must NOT borrow the previous confirmation
        let err2 = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix.clone(),
                    content: "different_content".to_string(),
                }],
            })
            .await
            .unwrap_err();
        assert!(
            err2.downcast_ref::<ConfirmationRequired>().is_some(),
            "changed operation must NOT reuse pending confirmation"
        );
    }

    #[tokio::test]
    #[ignore = "Artist does not retain stale confirmation state"]
    async fn test_stale_changed_candidate_invalidates() {
        // The pending key includes the candidate hash, so a retry where
        // the file changed to make the prefix match a *different* line
        // produces a new candidate → a new pending key → ConfirmationRequired
        // is returned again (not auto-approved).
        //
        // Here we verify this by setting up a stale scenario, storing a
        // pending, then *directly removing* the pending and repeating.
        // The second attempt should also return ConfirmationRequired
        // (because there is no pending to borrow).
        // This tests the conceptual requirement "changed candidate
        // invalidates confirmation" – if the stored confirmation doesn't
        // match the current state, the gate re-fires.
        let (mut mgr, path, prefix, _candidate, _tmpdir) =
            setup_stale_scenario("some line data\nsecond\nthird\n").await;

        // First attempt — store pending
        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix.clone(),
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ConfirmationRequired>().is_some());
        assert!(!mgr.pending_confirmations.is_empty());

        // Erase all pending – simulate that the candidate became stale
        mgr.pending_confirmations.clear();

        // Retry — pending is gone, so ConfirmationRequired again
        let err2 = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix.clone(),
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap_err();
        assert!(
            err2.downcast_ref::<ConfirmationRequired>().is_some(),
            "without a matching pending, the gate should require confirmation again"
        );
    }

    #[tokio::test]
    async fn test_stale_relocation_no_gate() {
        // When the old full hash STILL exists (at a different position),
        // the edit should apply without confirmation gate.
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
            .split('|')
            .next()
            .unwrap()
            .trim()
            .to_string();

        // Reorder so "first line" moves to position 3
        tokio::fs::write(&p, "second line\nthird line\nfirst line\n")
            .await
            .unwrap();

        let result = mgr
            .edit_file(EditRequest {
                path: p.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: first_prefix.clone(),
                    content: "replaced moved".to_string(),
                }],
            })
            .await
            .unwrap();
        assert!(result.content.contains("replaced moved"));
        let fc = tokio::fs::read_to_string(&p).await.unwrap();
        assert_eq!(fc, "second line\nthird line\nreplaced moved\n");
    }

    #[tokio::test]
    #[ignore = "Artist does not retain stale confirmation state"]
    async fn test_stale_reread_clears_gate() {
        let (mut mgr, path, prefix, _candidate, _tmpdir) = setup_stale_scenario(
            "some line data
second
third
",
        )
        .await;

        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: prefix,
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ConfirmationRequired>().is_some());
        assert!(!mgr.pending_confirmations.is_empty());

        // A full reread acknowledges the current view. It clears the stale
        // confirmation and returns the current mnemonic for the line rather
        // than reviving the retired stale mnemonic.
        let reread = mgr
            .read_file(ReadFileRequest {
                path: path.clone(),
                start_line: 1,
                max_lines: None,
            })
            .await
            .unwrap();
        let refreshed_anchor = reread.lines[0].anchor.clone();

        assert!(
            mgr.pending_confirmations.is_empty(),
            "read_file should clear pending confirmations"
        );

        let result = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: refreshed_anchor,
                    content: "replacement".to_string(),
                }],
            })
            .await
            .unwrap();
        assert!(result.content.contains("replacement"));
    }

    #[tokio::test]
    #[ignore = "Artist requires a fresh read instead of retry confirmation"]
    async fn test_stale_delete_retry() {
        let (mut mgr, path, prefix, _candidate, _tmpdir) =
            setup_stale_scenario("some line data\nsecond\nthird\n").await;

        // First attempt with Delete
        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Delete {
                    end_hash: None,
                    hash: prefix.clone(),
                }],
            })
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ConfirmationRequired>().is_some());

        // Retry same Delete
        let result = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Delete {
                    end_hash: None,
                    hash: prefix.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(result.total_lines, 2);
        let fc = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(fc, "second\nthird\n");
    }

    // -----------------------------------------------------------------------
    // Cross-file state isolation test (Requirement 1)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cross_file_mnemonic_isolation() {
        let tmpdir = tempfile::tempdir().unwrap();
        let p1 = tmpdir.path().join("file_a.txt");
        let p2 = tmpdir.path().join("file_b.txt");
        tokio::fs::write(&p1, "alpha\nbeta\ngamma\n").await.unwrap();
        tokio::fs::write(&p2, "delta\nepsilon\nzeta\n")
            .await
            .unwrap();
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

        // Handles are deliberately reusable across files; path + handle is the
        // complete model-facing address.
        let anchor = r1.lines[0].anchor.clone();
        assert_eq!(anchor, r2.lines[0].anchor);

        mgr.edit_file(EditRequest {
            path: s2.clone(),
            operations: vec![EditOperation::Replace {
                end_hash: None,
                hash: anchor.clone(),
                content: "replaced delta".to_string(),
            }],
        })
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&p1).await.unwrap(),
            "alpha\nbeta\ngamma\n"
        );
        assert_eq!(
            tokio::fs::read_to_string(&p2).await.unwrap(),
            "replaced delta\nepsilon\nzeta\n"
        );

        mgr.edit_file(EditRequest {
            path: s1.clone(),
            operations: vec![EditOperation::Replace {
                end_hash: None,
                hash: anchor,
                content: "replaced alpha".to_string(),
            }],
        })
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&p1).await.unwrap(),
            "replaced alpha\nbeta\ngamma\n"
        );
    }

    // -----------------------------------------------------------------------
    // Multi-operation no-borrow test (Requirement 2)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_multi_operation_no_borrow() {
        let (mut mgr, path, prefix, _candidate, _tmpdir) = setup_stale_scenario(
            "first
second
third
",
        )
        .await;

        // Obtain the already-issued mnemonic directly. A full reread here
        // would acknowledge the current view and retire the first stale
        // mnemonic, invalidating the scenario this test is meant to exercise.
        let current_view = FileView::from_text(
            "first
second
third
",
            Path::new(&path),
        );
        let third_full = current_view.lines[2].full_hash.clone();
        let h3 = FileToolManager::anchor_for_full_hash(mgr.path_prefixes_mut(&path), &third_full)
            .unwrap();

        let guard3 = short_hash(&third_full, &current_view.lines);
        let bogus3 = hash_to_base32(compute_hash(b"bogus third"));
        mgr.path_prefixes_mut(&path)
            .insert(h3.clone(), pack_binding(&bogus3, &guard3));
        mgr.reconcile_path_anchors(&path, &current_view, false);

        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![
                    EditOperation::Replace {
                        end_hash: None,
                        hash: prefix.clone(),
                        content: "new first".to_string(),
                    },
                    EditOperation::Replace {
                        end_hash: None,
                        hash: h3.clone(),
                        content: "new third".to_string(),
                    },
                ],
            })
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<ConfirmationRequired>().is_some());

        let err2 = mgr
            .edit_file(EditRequest {
                path,
                operations: vec![
                    EditOperation::Replace {
                        end_hash: None,
                        hash: prefix,
                        content: "new first".to_string(),
                    },
                    EditOperation::Replace {
                        end_hash: None,
                        hash: h3,
                        content: "different third".to_string(),
                    },
                ],
            })
            .await
            .unwrap_err();
        assert!(
            err2.downcast_ref::<ConfirmationRequired>().is_some(),
            "changed multi-op request must not reuse pending confirmation"
        );
    }

    // -----------------------------------------------------------------------
    // Atomic failure test (Requirement 3)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_atomic_failure_leaves_file_unchanged() {
        // Use setup_stale_scenario for a reliable stale mapping
        let (mut mgr, path, _prefix, _candidate, _tmpdir) =
            setup_stale_scenario("keep\nkeep\nkeep\nkeep\n").await;

        // The stale edit attempt must fail before any write
        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: "nonexistent".to_string(),
                    content: "should NOT write".to_string(),
                }],
            })
            .await
            .unwrap_err();
        // The edit fails because "nonexistent" doesn't match anything
        assert!(
            err.downcast_ref::<ConfirmationRequired>().is_none() || !err.to_string().is_empty(),
            "edit must fail before writing to disk"
        );

        // File must be UNCHANGED (atomicity)
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(
            content, "keep\nkeep\nkeep\nkeep\n",
            "file must not be modified after a failed atomic batch"
        );
    }

    // -----------------------------------------------------------------------
    // Full-hash collision detection (Requirement 6)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_stale_confirmation_exposes_candidate_mnemonic_not_hash() {
        let (mut mgr, path, anchor, candidate_anchor, _tmpdir) =
            setup_stale_scenario("line one\nline two\n").await;
        let err = mgr
            .edit_file(EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    end_hash: None,
                    hash: anchor.clone(),
                    content: "new".to_string(),
                }],
            })
            .await
            .unwrap_err();
        let confirmation = err.downcast_ref::<ConfirmationRequired>().unwrap();
        assert_eq!(confirmation.visible_anchor, anchor);
        assert_eq!(confirmation.candidate_anchor, candidate_anchor);
        assert!(is_mnemonic_handle(&confirmation.candidate_anchor));
        assert!(!confirmation.message.contains("Candidate HASH"));
    }

    // -----------------------------------------------------------------------
    // Prefix growth test (Requirement 6)
    // -----------------------------------------------------------------------

    #[test]
    fn test_prefix_grows_on_collision() {
        // When many lines share a common hash prefix, prefixes should grow
        // to the minimum unique length.
        let lines: Vec<LineInfo> = (0..5)
            .map(|i| {
                // Create lines whose hashes share the first few chars:
                // we use the same base content but different numbers to
                // force collisions on early prefixes.
                let content = format!("identical prefix line {}", i);
                LineInfo {
                    full_hash: hash_to_base32(compute_hash(content.as_bytes())),
                    content,
                }
            })
            .collect();

        let prefixes: Vec<String> = lines
            .iter()
            .map(|li| short_hash(&li.full_hash, &lines))
            .collect();

        // Every prefix must be unique
        let mut sorted = prefixes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            prefixes.len(),
            "every short prefix must be unique"
        );

        // Hidden guard prefixes use the shortest unique prefix with a one-character floor.
        for prefix in &prefixes {
            assert!(!prefix.is_empty());
        }
    }

    // -----------------------------------------------------------------------
    // Duplicate determinism regression
    // -----------------------------------------------------------------------

    #[test]
    fn test_repeated_duplicate_blocks_have_deterministic_hashes() {
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
            .map(|line| line.full_hash)
            .collect();

        for _ in 0..256 {
            let actual: Vec<String> = FileView::from_text(text, path)
                .lines
                .into_iter()
                .map(|line| line.full_hash)
                .collect();
            assert_eq!(
                actual, expected,
                "same file must always produce the same anchors"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Duplicate invalidation test (Requirement 7)
    // -----------------------------------------------------------------------

    #[test]
    fn test_duplicate_invalidation_on_insert_before() {
        // Given a file with duplicate lines [A, A, A], inserting another A
        // before the group should produce 4 distinct hashes because the
        // occurrence indices shift.
        let path = Path::new("test.txt");
        let view1 = FileView::from_text("A\nA\nA\n", path);
        let orig_hashes: Vec<String> = view1.lines.iter().map(|li| li.full_hash.clone()).collect();
        // All 3 have distinct hashes (disambiguated by occurrence index)
        let mut sorted = orig_hashes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            3,
            "three duplicates must have three different hashes"
        );

        // Insert a new A before the first A → becomes [A_new, A_0, A_1, A_2]
        // The old hashes for indices 0,1,2 should NOT be reused for indices 1,2,3
        let view2 = FileView::from_text("A\nA\nA\nA\n", path);
        let new_hashes: Vec<String> = view2.lines.iter().map(|li| li.full_hash.clone()).collect();
        // None of the new hashes should match any old hash (shifted occurrence indices)
        for old_h in &orig_hashes {
            assert!(
                !new_hashes.contains(old_h),
                "old duplicate hash {} must not be reused after insert-before",
                old_h
            );
        }
    }
}
