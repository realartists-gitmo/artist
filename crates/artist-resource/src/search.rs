use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use fff_search::{
    Constraint, ConstraintVec, FFFMode, FFFQuery, FilePicker, FilePickerOptions, FuzzyQuery,
    FuzzySearchOptions, GrepMode, GrepSearchOptions, MixedItemRef, PaginationArgs,
    SharedFilePicker, SharedFrecency,
};
use globset::Glob;
use serde_json::{Value, json};

use crate::ResourceUri;

/// The single native FFF index over the complete Artist mount.
pub struct SearchEngine {
    mount_root: PathBuf,
    picker: SharedFilePicker,
    frecency: SharedFrecency,
    generation: AtomicU64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedGrepMatch {
    pub uri: ResourceUri,
    pub line_number: u64,
    pub column: usize,
    pub text: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrepPage {
    pub matches: Vec<IndexedGrepMatch>,
    pub cursor: Option<String>,
}

impl SearchEngine {
    pub fn new(mount_root: impl Into<PathBuf>) -> Result<Self, String> {
        let mount_root = mount_root.into();
        let picker = SharedFilePicker::default();
        let frecency = SharedFrecency::default();
        build_picker(&mount_root, picker.clone(), frecency.clone())?;
        Ok(Self {
            mount_root,
            picker,
            frecency,
            generation: AtomicU64::new(0),
        })
    }

    pub fn refresh(&self) -> Result<(), String> {
        self.picker
            .trigger_full_rescan_async(&self.frecency)
            .map_err(|error| error.to_string())?;
        if !self.picker.wait_for_scan(Duration::from_secs(30)) {
            return Err("FFF rescan timed out".into());
        }
        Ok(())
    }

    pub fn synchronize(&self, generation: u64) -> Result<(), String> {
        if self.generation.load(Ordering::Acquire) != generation {
            self.refresh()?;
            self.generation.store(generation, Ordering::Release);
        }
        Ok(())
    }

    /// Canonical URIs and byte lengths currently held by the native index.
    pub fn indexed_resources(&self) -> Result<Vec<(ResourceUri, u64)>, String> {
        let guard = self.picker.read().map_err(|error| error.to_string())?;
        let picker = guard.as_ref().ok_or("FFF index is not initialized")?;
        picker
            .get_files()
            .iter()
            .map(|file| {
                let path = self.mount_root.join(file.relative_path(picker));
                Ok((mount_path_to_uri(&self.mount_root, &path)?, file.size))
            })
            .collect()
    }

    pub fn find(
        &self,
        uri: &ResourceUri,
        glob: Option<&str>,
        max_depth: Option<usize>,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Value, String> {
        let guard = self.picker.read().map_err(|error| error.to_string())?;
        let picker = guard.as_ref().ok_or("FFF index is not initialized")?;
        let root = uri_to_mount_path(&self.mount_root, uri)?;
        let root_rel = root
            .strip_prefix(&self.mount_root)
            .map_err(|_| "URI is outside the search mount")?;
        let offset = parse_cursor(cursor, "find")?;
        let pattern = match glob {
            Some(glob) if !root_rel.as_os_str().is_empty() => {
                format!("{}/{}", root_rel.to_string_lossy(), glob)
            }
            Some(glob) => glob.to_owned(),
            None => "**".into(),
        };
        let depth_pattern = max_depth.map(|depth| depth_glob(root_rel, depth));
        let mut constraints: ConstraintVec<'_> =
            std::iter::once(Constraint::Glob(pattern.as_str())).collect();
        if let Some(depth_pattern) = depth_pattern.as_deref() {
            constraints.push(Constraint::Glob(depth_pattern));
        }
        let query = FFFQuery {
            raw_query: &pattern,
            constraints,
            fuzzy_query: FuzzyQuery::Empty,
            location: None,
        };
        let result = picker.fuzzy_search_mixed(
            &query,
            None,
            FuzzySearchOptions {
                pagination: PaginationArgs {
                    offset,
                    limit: limit.saturating_add(1),
                },
                max_threads: 0,
                current_file: None,
                ..Default::default()
            },
        );
        let mut paths = result
            .items
            .into_iter()
            .map(|item| match item {
                MixedItemRef::File(item) => PathBuf::from(item.relative_path(picker)),
                MixedItemRef::Dir(item) => PathBuf::from(item.relative_path(picker)),
            })
            .filter(|path| path.starts_with(root_rel))
            .filter(|path| {
                !path
                    .strip_prefix(root_rel)
                    .unwrap_or(path)
                    .as_os_str()
                    .is_empty()
            })
            .collect::<Vec<_>>();
        let has_more = paths.len() > limit;
        paths.truncate(limit);
        let page = paths
            .iter()
            .map(|path| {
                mount_path_to_uri(&self.mount_root, &self.mount_root.join(path))
                    .map(|u| u.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let next = has_more.then(|| format!("find:{}", offset + page.len()));
        Ok(json!({"results": page, "cursor": next}))
    }

    pub fn grep(
        &self,
        uri: &ResourceUri,
        regex: &str,
        include_glob: Option<&str>,
        context: usize,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<GrepPage, String> {
        regex::Regex::new(regex).map_err(|error| format!("invalid regex: {error}"))?;
        // FFF's public grep entry point accepts an FFFQuery and its query type
        // strips a leading backslash before `*`, `/`, or `!`. Wrapping the
        // already-validated raw expression prevents that query-language escape
        // rule from touching the regex while preserving regex semantics.
        let raw_regex = format!("(?:{regex})");
        let guard = self.picker.read().map_err(|error| error.to_string())?;
        let picker = guard.as_ref().ok_or("FFF index is not initialized")?;
        let root = uri_to_mount_path(&self.mount_root, uri)?;
        let root_rel = root
            .strip_prefix(&self.mount_root)
            .map_err(|_| "URI is outside the search mount")?;
        let offset = parse_cursor(cursor, "grep")?;
        let include_pattern = match include_glob {
            Some(glob) if !root_rel.as_os_str().is_empty() => {
                format!("{}/{}", root_rel.to_string_lossy(), glob)
            }
            Some(glob) => glob.to_owned(),
            None => "**".into(),
        };
        let include = Glob::new(&include_pattern)
            .map_err(|error| format!("invalid include glob: {error}"))?
            .compile_matcher();
        // FFF 0.10.5 deliberately retries a zero-result constrained grep with
        // its constraints removed. Artist URI roots and include globs are hard
        // constraints, so feed FFF only the raw regex and apply scope to each
        // bounded native page. This keeps FFF's stable file-offset pagination
        // without allowing its query-language fallback to escape the URI root.
        let query = FFFQuery {
            raw_query: &raw_regex,
            constraints: ConstraintVec::new(),
            fuzzy_query: FuzzyQuery::Text(&raw_regex),
            location: None,
        };
        let mut matches = Vec::new();
        let mut page_offset = offset;
        let next = loop {
            let result = picker.grep(
                &query,
                &GrepSearchOptions {
                    mode: GrepMode::Regex,
                    file_offset: page_offset,
                    page_limit: limit.saturating_sub(matches.len()).max(1),
                    before_context: context,
                    after_context: context,
                    ..Default::default()
                },
            );
            if let Some(error) = result.regex_fallback_error {
                return Err(format!("invalid regex: {error}"));
            }
            for found in result.matches {
                let file = result.files[found.file_index];
                let relative_text = file.relative_path(picker);
                let relative = Path::new(&relative_text);
                if !relative.starts_with(root_rel) || !include.is_match(relative) {
                    continue;
                }
                let path = self.mount_root.join(relative);
                matches.push(IndexedGrepMatch {
                    uri: mount_path_to_uri(&self.mount_root, &path)?,
                    line_number: found.line_number,
                    column: found.col,
                    text: found.line_content,
                    before: found.context_before,
                    after: found.context_after,
                });
            }
            if matches.len() >= limit || result.next_file_offset == 0 {
                break (result.next_file_offset != 0)
                    .then(|| format!("grep:{}", result.next_file_offset));
            }
            page_offset = result.next_file_offset;
        };
        Ok(GrepPage {
            matches,
            cursor: next,
        })
    }
}

fn depth_glob(root: &Path, depth: usize) -> String {
    if depth == 0 {
        return "__artist_no_descendants__".into();
    }
    let prefix = (!root.as_os_str().is_empty()).then(|| format!("{}/", root.to_string_lossy()));
    let patterns = (1..=depth)
        .map(|level| {
            format!(
                "{}{}",
                prefix.as_deref().unwrap_or_default(),
                std::iter::repeat_n("*", level)
                    .collect::<Vec<_>>()
                    .join("/")
            )
        })
        .collect::<Vec<_>>();
    if patterns.len() == 1 {
        patterns.into_iter().next().unwrap()
    } else {
        format!("{{{}}}", patterns.join(","))
    }
}

fn build_picker(
    root: &Path,
    picker: SharedFilePicker,
    frecency: SharedFrecency,
) -> Result<(), String> {
    FilePicker::new_with_shared_state(
        picker.clone(),
        frecency,
        FilePickerOptions {
            base_path: root.to_string_lossy().into_owned(),
            mode: FFFMode::Ai,
            watch: true,
            enable_mmap_cache: false,
            enable_content_indexing: false,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    if !picker.wait_for_indexing_complete(Duration::from_secs(30)) {
        return Err("FFF initial scan timed out".into());
    }
    if !picker.wait_for_watcher(Duration::from_secs(30)) {
        return Err("FFF watcher startup timed out".into());
    }
    Ok(())
}

fn parse_cursor(cursor: Option<&str>, kind: &str) -> Result<usize, String> {
    let Some(cursor) = cursor else { return Ok(0) };
    cursor
        .strip_prefix(&format!("{kind}:"))
        .ok_or_else(|| "invalid continuation cursor".to_owned())?
        .parse()
        .map_err(|_| "invalid continuation cursor".into())
}

/// Convert a canonical URI to its path in the FUSE projection.
pub fn uri_to_mount_path(mount: &Path, uri: &ResourceUri) -> Result<PathBuf, String> {
    let mut path = mount.join(uri.as_url().scheme());
    if uri.as_url().scheme() == "file" {
        path.extend(
            uri.file_path()
                .ok_or("invalid file URI")?
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(v) => Some(v),
                    _ => None,
                }),
        );
    } else {
        if let Some(host) = uri.as_url().host_str() {
            path.push(host);
        }
        path.extend(uri.as_url().path_segments().into_iter().flatten());
    }
    let projection = uri.projection_segments();
    if let Some(first) = projection.first() {
        let name = path
            .file_name()
            .ok_or("projected URI has no base name")?
            .to_string_lossy();
        path.set_file_name(format!("{name}?{first}"));
        path.extend(&projection[1..]);
    }
    Ok(path)
}

/// Convert a FUSE projection path back to a canonical URI.
pub fn mount_path_to_uri(mount: &Path, path: &Path) -> Result<ResourceUri, String> {
    let relative = path
        .strip_prefix(mount)
        .map_err(|_| "path is outside Artist mount")?;
    let mut components = relative.components().filter_map(|c| match c {
        std::path::Component::Normal(v) => Some(v.to_string_lossy().into_owned()),
        _ => None,
    });
    let scheme = components.next().ok_or("mount path has no scheme")?;
    let mut base = Vec::new();
    let mut projection = Vec::new();
    let mut projected = false;
    for component in components {
        if !projected {
            if let Some((name, first)) = component.split_once('?') {
                base.push(name.to_owned());
                projection.push(first.to_owned());
                projected = true;
            } else {
                base.push(component);
            }
        } else {
            projection.push(component);
        }
    }
    let text = if matches!(scheme.as_str(), "file" | "profiles" | "plugins") {
        format!("{scheme}:///{}", base.join("/"))
    } else if base.is_empty() {
        format!("{scheme}:/")
    } else {
        format!("{scheme}://{}", base.join("/"))
    };
    let mut uri = ResourceUri::resolve(&text, Path::new("/")).map_err(|e| e.to_string())?;
    for segment in projection {
        uri = uri
            .descend_projection(&segment)
            .map_err(|e| e.to_string())?;
    }
    Ok(uri)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_path_round_trip() {
        let mount = Path::new("/mnt/artist");
        let uri = ResourceUri::resolve("file:///work/rust.rs?symbols/foo/callers", Path::new("/"))
            .unwrap();
        let path = uri_to_mount_path(mount, &uri).unwrap();
        assert_eq!(
            path,
            Path::new("/mnt/artist/file/work/rust.rs?symbols/foo/callers")
        );
        assert_eq!(mount_path_to_uri(mount, &path).unwrap(), uri);
    }

    #[test]
    fn hosted_scheme_path_round_trip() {
        let mount = Path::new("/mnt/artist");
        let uri =
            ResourceUri::resolve("process://run-7/stdout?chunks/latest", Path::new("/")).unwrap();
        let path = uri_to_mount_path(mount, &uri).unwrap();
        assert_eq!(
            path,
            Path::new("/mnt/artist/process/run-7/stdout?chunks/latest")
        );
        assert_eq!(mount_path_to_uri(mount, &path).unwrap(), uri);
    }

    #[test]
    fn hostless_plugin_scheme_path_round_trip() {
        let mount = Path::new("/mnt/artist");
        let uri = ResourceUri::resolve("plugins:///tool-read/src/lib.rs", Path::new("/")).unwrap();
        let path = uri_to_mount_path(mount, &uri).unwrap();
        assert_eq!(path, Path::new("/mnt/artist/plugins/tool-read/src/lib.rs"));
        assert_eq!(mount_path_to_uri(mount, &path).unwrap(), uri);
    }

    #[test]
    fn one_fff_index_finds_and_greps_canonical_resources() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file/workspace/src/lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "pub fn indexed_symbol() {} // projected_body\n").unwrap();
        std::fs::write(
            file.with_file_name("other.rs"),
            "pub fn indexed_symbol_too() {}\n*.rs !excluded type:rust path/to/file\n",
        )
        .unwrap();
        let projected_file = file.with_file_name("lib.rs?symbols").join("foo");
        std::fs::create_dir_all(projected_file.parent().unwrap()).unwrap();
        std::fs::write(&projected_file, "projected_body\n").unwrap();
        let index = SearchEngine::new(temp.path()).unwrap();
        let root = ResourceUri::resolve("file:///workspace", Path::new("/")).unwrap();
        let found = index.find(&root, Some("**/*.rs"), None, None, 50).unwrap();
        assert_eq!(found["results"].as_array().unwrap().len(), 2);
        let matches = index
            .grep(&root, "indexed_symbol", Some("**/*.rs"), 0, None, 100)
            .unwrap();
        assert_eq!(
            matches.matches[0].uri.to_string(),
            "file:///workspace/src/lib.rs"
        );
        assert_eq!(matches.matches[0].line_number, 1);

        let first = index
            .grep(&root, "indexed_symbol", Some("**/*.rs"), 0, None, 1)
            .unwrap();
        assert_eq!(first.cursor.as_deref(), Some("grep:1"));
        let second = index
            .grep(
                &root,
                "indexed_symbol",
                Some("**/*.rs"),
                0,
                first.cursor.as_deref(),
                1,
            )
            .unwrap();
        assert_ne!(first.matches[0].uri, second.matches[0].uri);
        if let Some(cursor) = second.cursor.as_deref() {
            let terminal = index
                .grep(&root, "indexed_symbol", Some("**/*.rs"), 0, Some(cursor), 1)
                .unwrap();
            assert!(terminal.matches.is_empty());
            assert!(terminal.cursor.is_none());
        }

        let syntax = index
            .grep(
                &root,
                r"\*\.rs.*type:rust.*path/to/file",
                Some("**/*.rs"),
                0,
                None,
                10,
            )
            .unwrap();
        assert_eq!(syntax.matches.len(), 1);
        assert!(index.grep(&root, "[", None, 0, None, 10).is_err());

        let projected_root =
            ResourceUri::resolve("file:///workspace/src/lib.rs?symbols", Path::new("/")).unwrap();
        let projected = index
            .grep(&projected_root, "projected_body", None, 0, None, 10)
            .unwrap();
        assert_eq!(projected.matches.len(), 1);
        assert_eq!(
            projected.matches[0].uri.to_string(),
            "file:///workspace/src/lib.rs?symbols/foo"
        );
    }

    #[test]
    fn ordinary_edits_are_discovered_by_the_long_running_watcher() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("file/workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let index = SearchEngine::new(temp.path()).unwrap();
        let root = ResourceUri::resolve("file:///workspace", Path::new("/")).unwrap();

        std::fs::write(
            workspace.join("watched.rs"),
            "fn appeared_after_indexing() {}\n",
        )
        .unwrap();

        for _ in 0..100 {
            let found = index
                .find(&root, Some("watched.rs"), None, None, 10)
                .unwrap();
            if found["results"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
            {
                let matches = index
                    .grep(&root, "appeared_after_indexing", None, 0, None, 10)
                    .unwrap();
                assert_eq!(matches.matches.len(), 1);
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        panic!("FFF watcher did not discover an ordinary filesystem edit");
    }
}
