//! The kernel's shared FFF-backed filesystem search surface.

use crate::{
    Anchor, AnchorSet, AnchoredLine, AnchoredText, KernelError, LineEnding, ResourceUri,
    StructuralAnalyzer,
};
use fff_search::{
    FFFMode, FilePicker, FilePickerOptions, FuzzySearchOptions, GrepSearchOptions, PaginationArgs,
    SharedFilePicker, SharedFrecency, parse_grep_query,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

fn picker(root: &Path) -> Result<(SharedFilePicker, PathBuf), KernelError> {
    let shared = SharedFilePicker::default();
    FilePicker::new_with_shared_state(
        shared.clone(),
        SharedFrecency::noop(),
        FilePickerOptions {
            base_path: root.display().to_string(),
            mode: FFFMode::Ai,
            watch: false,
            enable_mmap_cache: false,
            enable_content_indexing: false,
            ..Default::default()
        },
    )
    .map_err(|error| KernelError::Handler {
        message: format!("initialize fff index: {error}"),
    })?;
    if !shared.wait_for_indexing_complete(Duration::from_secs(30)) {
        return Err(KernelError::Handler {
            message: "fff indexing timed out".to_owned(),
        });
    }
    Ok((shared, root.to_owned()))
}

fn absolute_files(
    shared: &SharedFilePicker,
    root: &Path,
    query: &str,
) -> Result<Vec<PathBuf>, KernelError> {
    let guard = shared.read().map_err(|error| KernelError::Handler {
        message: format!("read fff index: {error}"),
    })?;
    let picker = guard.as_ref().ok_or_else(|| KernelError::Handler {
        message: "fff index was dropped".to_owned(),
    })?;
    let parsed = fff_search::QueryParser::default().parse(query);
    let result = picker.fuzzy_search(
        &parsed,
        None,
        FuzzySearchOptions {
            pagination: PaginationArgs {
                offset: 0,
                limit: usize::MAX,
            },
            ..Default::default()
        },
    );
    Ok(result
        .items
        .into_iter()
        .map(|item| item.absolute_path(picker, root))
        .collect())
}

pub fn find(root: &Path, query: &str) -> Result<Vec<PathBuf>, KernelError> {
    if root.is_file() {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        return Ok((query.is_empty() || name.contains(query))
            .then(|| root.to_owned())
            .into_iter()
            .collect());
    }
    let (shared, root) = picker(root)?;
    absolute_files(&shared, &root, query)
}

pub fn grep(
    root: &Path,
    pattern: &str,
    structure: &StructuralAnalyzer,
) -> Result<Vec<AnchoredText>, KernelError> {
    let requested_file = root.is_file().then(|| root.to_owned());
    let index_root = root.parent().unwrap_or(root);
    let (shared, root) = picker(index_root)?;
    let guard = shared.read().map_err(|error| KernelError::Handler {
        message: format!("read fff index: {error}"),
    })?;
    let picker = guard.as_ref().ok_or_else(|| KernelError::Handler {
        message: "fff index was dropped".to_owned(),
    })?;
    let query = parse_grep_query(pattern);
    let result = picker.grep(
        &query,
        &GrepSearchOptions {
            page_limit: usize::MAX,
            ..Default::default()
        },
    );
    let mut grouped = BTreeMap::<PathBuf, Vec<AnchoredLine>>::new();
    for hit in result.matches {
        let file = result.files[hit.file_index].absolute_path(picker, &root);
        if requested_file
            .as_ref()
            .is_some_and(|requested| requested != &file)
        {
            continue;
        }
        let source = std::fs::read(&file).map_err(|error| KernelError::Handler {
            message: format!("read {}: {error}", file.display()),
        })?;
        let (_, lines) = structure.analyze(&file, &source);
        let index = hit.line_number.saturating_sub(1) as usize;
        let anchor_set = AnchorSet::from_inputs(
            &lines
                .iter()
                .map(|line| line.anchor_input())
                .collect::<Vec<_>>(),
        )
        .ok();
        let anchor = lines
            .get(index)
            .and_then(|_| {
                anchor_set
                    .as_ref()?
                    .items()
                    .get(index)
                    .map(|item| item.anchor.clone())
            })
            .unwrap_or_else(|| Anchor::from_tokens(vec![(hit.line_number).to_string()]));
        let ending = if hit.line_content.ends_with('\r') {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        };
        grouped.entry(file).or_default().push(AnchoredLine {
            anchor,
            text: hit.line_content,
            ending,
        });
    }
    grouped
        .into_iter()
        .map(|(path, lines)| {
            Ok(AnchoredText {
                uri: ResourceUri::parse(&path.display().to_string())?,
                lines,
            })
        })
        .collect()
}
