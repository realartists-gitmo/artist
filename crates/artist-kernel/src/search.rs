//! Artist-owned search semantics and resident filesystem search substrate.
//!
//! FFF is deliberately hidden behind this module. The public types here are
//! Artist types; FFF only supplies indexing and matching machinery.

use crate::{
    AnchorSet, AnchoredLine, AnchoredText, KernelError, LineEnding, ResourceUri,
    StructuralAnalyzer, StructuralLine,
};
use fff_search::{FFFMode, FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency};
use regex::Regex;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    Literal(String),
    Regex(String),
    Fuzzy(String),
}

impl Pattern {
    pub fn parse(input: &str) -> Result<Self, KernelError> {
        let (kind, payload) = if let Some(value) = input.strip_prefix("lit:") {
            ("literal", value)
        } else if let Some(value) = input.strip_prefix("re:") {
            ("regex", value)
        } else if let Some(value) = input.strip_prefix("fz:") {
            ("fuzzy", value)
        } else {
            ("fuzzy", input)
        };
        if payload.is_empty() {
            return Err(KernelError::InvalidPattern {
                message: "pattern payload cannot be empty".to_owned(),
            });
        }
        if kind == "regex" {
            Regex::new(payload).map_err(|error| KernelError::InvalidPattern {
                message: error.to_string(),
            })?;
        }
        Ok(match kind {
            "regex" => Self::Regex(payload.to_owned()),
            "fuzzy" => Self::Fuzzy(payload.to_owned()),
            _ => Self::Literal(payload.to_owned()),
        })
    }
}

#[derive(Clone)]
struct ResidentIndex {
    root: PathBuf,
    shared: SharedFilePicker,
}

#[derive(Clone, Default)]
pub struct SearchService {
    indexes: Arc<RwLock<BTreeMap<PathBuf, ResidentIndex>>>,
}

impl SearchService {
    pub fn new() -> Self {
        Self::default()
    }

    fn index(&self, root: &Path) -> Result<ResidentIndex, KernelError> {
        let root = std::fs::canonicalize(root).map_err(|error| KernelError::Handler {
            message: format!("canonicalize search root {}: {error}", root.display()),
        })?;
        if !root.is_dir() {
            return Err(KernelError::WrongKind {
                message: format!("search root is not a directory: {}", root.display()),
            });
        }
        if let Some(index) = self
            .indexes
            .read()
            .map_err(|_| KernelError::Handler {
                message: "search index lock poisoned".to_owned(),
            })?
            .get(&root)
            .cloned()
        {
            return Ok(index);
        }
        let shared = SharedFilePicker::default();
        FilePicker::new_with_shared_state(
            shared.clone(),
            SharedFrecency::noop(),
            FilePickerOptions {
                base_path: root.display().to_string(),
                mode: FFFMode::Ai,
                watch: true,
                enable_mmap_cache: false,
                enable_content_indexing: false,
                ..Default::default()
            },
        )
        .map_err(|error| KernelError::Handler {
            message: format!("initialize search index: {error}"),
        })?;
        if !shared.wait_for_indexing_complete(Duration::from_secs(30)) {
            return Err(KernelError::Handler {
                message: "search index initialization timed out".to_owned(),
            });
        }
        if !shared.wait_for_watcher(Duration::from_secs(30)) {
            return Err(KernelError::Handler {
                message: "search watcher initialization timed out".to_owned(),
            });
        }
        let index = ResidentIndex {
            root: root.clone(),
            shared,
        };
        self.indexes
            .write()
            .map_err(|_| KernelError::Handler {
                message: "search index lock poisoned".to_owned(),
            })?
            .insert(root, index.clone());
        Ok(index)
    }

    pub fn find_files(&self, root: &Path, pattern: &Pattern) -> Result<Vec<PathBuf>, KernelError> {
        if root.is_file() {
            let candidate = root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let matched = match pattern {
                Pattern::Literal(query) => candidate.contains(query),
                Pattern::Regex(query) => Regex::new(query)
                    .map_err(|error| KernelError::InvalidPattern {
                        message: error.to_string(),
                    })?
                    .is_match(&candidate),
                Pattern::Fuzzy(query) => fff_search::fuzzy_match_score(query, &candidate).is_some(),
            };
            return Ok(matched.then(|| root.to_owned()).into_iter().collect());
        }
        let index = self.index(root)?;
        let guard = index.shared.read().map_err(|error| KernelError::Handler {
            message: error.to_string(),
        })?;
        let picker = guard.as_ref().ok_or_else(|| KernelError::Handler {
            message: "search index unavailable".to_owned(),
        })?;
        let mut candidates = picker
            .get_files()
            .iter()
            .filter(|item| !item.is_deleted())
            .map(|item| {
                let path = item.absolute_path(picker, &index.root);
                let name = path
                    .strip_prefix(&index.root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                (path, name)
            })
            .collect::<Vec<_>>();
        candidates.extend(
            picker
                .get_dirs()
                .iter()
                .filter(|item| !item.is_deleted())
                .map(|item| {
                    let path = item.absolute_path(picker, &index.root);
                    let name = item.relative_path(picker);
                    (path, name)
                }),
        );
        let mut paths: Vec<PathBuf> = match pattern {
            Pattern::Fuzzy(query) => {
                let mut scored = candidates
                    .iter()
                    .filter_map(|(path, candidate)| {
                        fff_search::fuzzy_match_score(query, candidate).map(|score| (score, path))
                    })
                    .collect::<Vec<_>>();
                scored
                    .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
                scored.into_iter().map(|(_, path)| path.clone()).collect()
            }
            Pattern::Literal(query) => candidates
                .iter()
                .filter_map(|(path, candidate)| candidate.contains(query).then_some(path.clone()))
                .collect(),
            Pattern::Regex(query) => {
                let regex = Regex::new(query).map_err(|error| KernelError::InvalidPattern {
                    message: error.to_string(),
                })?;
                candidates
                    .iter()
                    .filter_map(|(path, candidate)| {
                        regex.is_match(candidate).then_some(path.clone())
                    })
                    .collect()
            }
        };
        let mut seen = std::collections::HashSet::new();
        paths.retain(|path| seen.insert(path.clone()));
        Ok(paths)
    }

    pub fn grep_file(
        &self,
        file: &Path,
        pattern: &Pattern,
        structure: &StructuralAnalyzer,
    ) -> Result<Vec<AnchoredText>, KernelError> {
        let root = if file.is_dir() {
            file
        } else {
            file.parent().unwrap_or(file)
        };
        let index = self.index(root)?;
        let guard = index.shared.read().map_err(|error| KernelError::Handler {
            message: error.to_string(),
        })?;
        let picker = guard.as_ref().ok_or_else(|| KernelError::Handler {
            message: "search index unavailable".to_owned(),
        })?;
        let source_paths = if file.is_file() {
            vec![(file.to_path_buf(), None)]
        } else {
            picker
                .get_files()
                .iter()
                .filter(|item| !item.is_deleted())
                .map(|item| (item.absolute_path(picker, &index.root), Some(())))
                .filter(|(path, _)| file.is_dir() && path.starts_with(file))
                .collect()
        };
        let source_before = source_paths
            .into_iter()
            .map(|(path, indexed)| {
                let source = match indexed {
                    // An explicitly addressed file is authorized by the
                    // caller and must not inherit FFF's ignore/index policy.
                    None => std::fs::read(&path).map_err(|error| KernelError::Handler {
                        message: format!("read {}: {error}", path.display()),
                    })?,
                    Some(()) => {
                        picker
                            .snapshot_file(&path)
                            .ok_or_else(|| KernelError::NotFound {
                                uri: path.display().to_string(),
                            })?
                    }
                };
                Ok((path, source))
            })
            .collect::<Result<BTreeMap<_, _>, KernelError>>()?;
        // Search the bytes captured above, not the live filesystem. This is
        // the strict snapshot boundary: matching and anchor derivation both
        // consume exactly the same byte sequence, so an A→B→A edit cannot
        // silently produce anchors for bytes FFF never searched.
        let mut grouped = BTreeMap::<PathBuf, Vec<usize>>::new();
        for (path, source) in &source_before {
            let (query, mode) = match pattern {
                Pattern::Literal(value) => (value.as_str(), fff_search::GrepMode::PlainText),
                Pattern::Regex(value) => (value.as_str(), fff_search::GrepMode::Regex),
                Pattern::Fuzzy(value) => (value.as_str(), fff_search::GrepMode::Fuzzy),
            };
            let hits = fff_search::grep_snapshot(source, query, mode)
                .map_err(|message| KernelError::InvalidPattern { message })?;
            let indices = hits
                .into_iter()
                .map(|hit| hit.line_number.saturating_sub(1) as usize)
                .collect::<Vec<_>>();
            if !indices.is_empty() {
                grouped.insert(path.clone(), indices);
            }
        }
        grouped
            .into_iter()
            .map(|(path, indices)| {
                let source = source_before
                    .get(&path)
                    .ok_or_else(|| KernelError::NotFound {
                        uri: path.display().to_string(),
                    })?;
                verify_snapshot(&path, source)?;
                let (_, lines) = structure.analyze(&path, source);
                let anchors = AnchorSet::from_inputs(
                    &lines
                        .iter()
                        .map(StructuralLine::anchor_input)
                        .collect::<Vec<_>>(),
                );
                Ok(AnchoredText {
                    uri: ResourceUri::parse(&path.display().to_string())?,
                    lines: indices
                        .into_iter()
                        .filter_map(|index| {
                            lines.get(index).and_then(|line| {
                                Some(AnchoredLine {
                                    anchor: anchors
                                        .as_ref()
                                        .ok()?
                                        .items()
                                        .get(index)?
                                        .anchor
                                        .clone(),
                                    text: String::from_utf8_lossy(&line.line_text).into_owned(),
                                    ending: line_ending(source, line.end_byte),
                                })
                            })
                        })
                        .collect(),
                })
            })
            .collect()
    }

    pub fn grep_text(
        texts: &[AnchoredText],
        pattern: &Pattern,
    ) -> Result<Vec<AnchoredText>, KernelError> {
        let regex = match pattern {
            Pattern::Regex(value) => {
                Some(
                    Regex::new(value).map_err(|error| KernelError::InvalidPattern {
                        message: error.to_string(),
                    })?,
                )
            }
            _ => None,
        };
        Ok(texts
            .iter()
            .filter_map(|text| {
                let lines = text
                    .lines
                    .iter()
                    .filter(|line| match pattern {
                        Pattern::Literal(value) => line.text.contains(value),
                        Pattern::Regex(_) => regex
                            .as_ref()
                            .is_some_and(|value| value.is_match(&line.text)),
                        Pattern::Fuzzy(value) => fff_search::fuzzy_line_matches(value, &line.text),
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                (!lines.is_empty()).then(|| AnchoredText {
                    uri: text.uri.clone(),
                    lines,
                })
            })
            .collect())
    }
}

fn verify_snapshot(path: &Path, expected: &[u8]) -> Result<(), KernelError> {
    let actual = std::fs::read(path).map_err(|error| KernelError::Handler {
        message: format!("read {}: {error}", path.display()),
    })?;
    if actual != expected {
        return Err(KernelError::Conflict {
            uri: ResourceUri::parse(&path.display().to_string())?.to_string(),
        });
    }
    Ok(())
}

fn line_ending(source: &[u8], end: usize) -> LineEnding {
    if source
        .get(end..)
        .is_some_and(|tail| tail.starts_with(b"\r\n"))
    {
        LineEnding::Crlf
    } else if source
        .get(end..)
        .is_some_and(|tail| tail.starts_with(b"\n"))
    {
        LineEnding::Lf
    } else if source
        .get(end..)
        .is_some_and(|tail| tail.starts_with(b"\r"))
    {
        LineEnding::Cr
    } else {
        LineEnding::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn parses_artist_patterns_with_fuzzy_default() {
        assert_eq!(Pattern::parse("foo").unwrap(), Pattern::Fuzzy("foo".into()));
        assert_eq!(
            Pattern::parse("lit:re:foo").unwrap(),
            Pattern::Literal("re:foo".into())
        );
        assert_eq!(
            Pattern::parse("lit:fz:foo").unwrap(),
            Pattern::Literal("fz:foo".into())
        );
        assert_eq!(
            Pattern::parse("lit:lit:foo").unwrap(),
            Pattern::Literal("lit:foo".into())
        );
        assert_eq!(
            Pattern::parse("re:foo.*bar").unwrap(),
            Pattern::Regex("foo.*bar".into())
        );
        assert_eq!(
            Pattern::parse("*.rs").unwrap(),
            Pattern::Fuzzy("*.rs".into())
        );
        assert_eq!(
            Pattern::parse("git:modified").unwrap(),
            Pattern::Fuzzy("git:modified".into())
        );
        for input in ["", "lit:", "re:", "fz:"] {
            assert!(matches!(
                Pattern::parse(input),
                Err(KernelError::InvalidPattern { .. })
            ));
        }
        assert!(matches!(
            Pattern::parse("re:("),
            Err(KernelError::InvalidPattern { .. })
        ));
        assert!(matches!(
            Pattern::parse("fz:"),
            Err(KernelError::InvalidPattern { .. })
        ));
    }

    #[test]
    fn resident_index_tracks_filesystem_changes() {
        let root = tempdir().unwrap();
        let service = SearchService::new();
        let pattern = Pattern::parse("after.txt").unwrap();
        assert!(
            service
                .find_files(root.path(), &pattern)
                .unwrap()
                .is_empty()
        );

        let path = root.path().join("after.txt");
        std::fs::write(&path, "created\n").unwrap();
        let mut found = false;
        for _ in 0..40 {
            if service.find_files(root.path(), &pattern).unwrap() == vec![path.clone()] {
                found = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(found, "resident index did not observe file creation");

        std::fs::remove_file(&path).unwrap();
        for _ in 0..40 {
            if service
                .find_files(root.path(), &pattern)
                .unwrap()
                .is_empty()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            service
                .find_files(root.path(), &pattern)
                .unwrap()
                .is_empty()
        );

        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        // Let FFF receive and install the new-directory watch before the
        // child creation, avoiding a deliberate create-during-registration
        // race in this watcher conformance test.
        std::thread::sleep(Duration::from_millis(250));
        let nested_file = nested.join("new.txt");
        std::fs::write(&nested_file, "nested\n").unwrap();
        let nested_pattern = Pattern::parse("new.txt").unwrap();
        let mut found_nested = false;
        for _ in 0..200 {
            if service.find_files(root.path(), &nested_pattern).unwrap()
                == vec![nested_file.clone()]
            {
                found_nested = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(found_nested, "resident index did not observe new directory");
        std::fs::remove_dir_all(&nested).unwrap();
    }

    #[test]
    fn fuzzy_find_preserves_score_order_and_all_patterns_use_relative_names() {
        let root = tempdir().unwrap();
        let names = ["zz_answer.rs", "answer.rs", "src_answer.rs"];
        for name in names {
            std::fs::write(root.path().join(name), "x\n").unwrap();
        }
        let service = SearchService::new();
        let pattern = Pattern::parse("fz:answer").unwrap();
        let mut found = None;
        for _ in 0..80 {
            let paths = service.find_files(root.path(), &pattern).unwrap();
            if paths.len() == names.len() {
                found = Some(paths);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let paths = found.expect("resident index did not observe fuzzy fixtures");
        let mut expected = names
            .iter()
            .filter_map(|name| {
                fff_search::fuzzy_match_score("answer", name)
                    .map(|score| (score, root.path().join(name)))
            })
            .collect::<Vec<_>>();
        expected.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        assert_eq!(
            paths,
            expected
                .into_iter()
                .map(|(_, path)| path)
                .collect::<Vec<_>>()
        );

        let absolute_query = Pattern::parse(&format!("lit:{}", root.path().display())).unwrap();
        assert!(
            service
                .find_files(root.path(), &absolute_query)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn grep_reconstructs_exact_line_endings() {
        let root = tempdir().unwrap();
        let structure = StructuralAnalyzer::default();
        let service = SearchService::new();
        let cases = [
            ("lf.txt", b"first\nneedle\n".as_slice(), LineEnding::Lf),
            (
                "crlf.txt",
                b"first\r\nneedle\r\n".as_slice(),
                LineEnding::Crlf,
            ),
            ("cr.txt", b"first\rneedle\r".as_slice(), LineEnding::Cr),
            ("none.txt", b"first\nneedle".as_slice(), LineEnding::None),
        ];
        for (name, bytes, _) in &cases {
            let path = root.path().join(name);
            std::fs::write(&path, bytes).unwrap();
        }
        for (name, _, ending) in &cases {
            let path = root.path().join(name);
            let matches = service
                .grep_file(&path, &Pattern::parse("needle").unwrap(), &structure)
                .unwrap();
            assert_eq!(matches.len(), 1, "{name}");
            assert_eq!(matches[0].lines[0].text, "needle", "{name}");
            assert_eq!(matches[0].lines[0].ending, *ending, "{name}");
        }
    }

    #[test]
    fn stale_search_snapshot_is_rejected_before_anchor_conversion() {
        let root = tempdir().unwrap();
        let path = root.path().join("race.txt");
        let snapshot = b"version A\nneedle\n".to_vec();
        std::fs::write(&path, &snapshot).unwrap();
        std::fs::write(&path, b"version B\nunrelated\n").unwrap();
        assert!(matches!(
            verify_snapshot(&path, &snapshot),
            Err(KernelError::Conflict { .. })
        ));
    }
}
