use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use fff_search::{
    FFFMode, FilePicker, FilePickerOptions, FuzzySearchOptions, GrepMode, GrepSearchOptions,
    PaginationArgs, grep::parse_grep_query,
};
use globset::Glob;
use serde_json::{Value, json};

use crate::ResourceUri;

/// The single native FFF index over the complete Artist mount.
pub struct SearchEngine {
    mount_root: PathBuf,
    picker: Mutex<FilePicker>,
    generation: AtomicU64,
}

impl SearchEngine {
    pub fn new(mount_root: impl Into<PathBuf>) -> Result<Self, String> {
        let mount_root = mount_root.into();
        let picker = build_picker(&mount_root)?;
        Ok(Self {
            mount_root,
            picker: Mutex::new(picker),
            generation: AtomicU64::new(0),
        })
    }

    pub fn refresh(&self) -> Result<(), String> {
        *self.picker.lock().map_err(|_| "FFF index lock poisoned")? =
            build_picker(&self.mount_root)?;
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
        let picker = self.picker.lock().map_err(|_| "FFF index lock poisoned")?;
        picker
            .get_files()
            .iter()
            .map(|file| {
                let path = self.mount_root.join(file.relative_path(&*picker));
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
        let picker = self.picker.lock().map_err(|_| "FFF index lock poisoned")?;
        let root = uri_to_mount_path(&self.mount_root, uri)?;
        let root_rel = root
            .strip_prefix(&self.mount_root)
            .map_err(|_| "URI is outside the search mount")?;
        let offset = parse_cursor(cursor)?;
        let matcher = glob
            .map(Glob::new)
            .transpose()
            .map_err(|e| e.to_string())?
            .map(|g| g.compile_matcher());
        let pattern = match glob {
            Some(glob) if !root_rel.as_os_str().is_empty() => {
                format!("{}/{}", root_rel.to_string_lossy(), glob)
            }
            Some(glob) => glob.to_owned(),
            None => "**".into(),
        };
        let result = picker.glob(
            &pattern,
            FuzzySearchOptions {
                pagination: PaginationArgs {
                    offset: 0,
                    limit: usize::MAX,
                },
                max_threads: 0,
                current_file: None,
                ..Default::default()
            },
        );
        let mut paths = result
            .items
            .into_iter()
            .map(|item| PathBuf::from(item.relative_path(&*picker)))
            .filter(|path| path.starts_with(root_rel))
            .filter(|path| {
                let relative = path.strip_prefix(root_rel).unwrap_or(path);
                !relative.as_os_str().is_empty()
                    && max_depth.is_none_or(|depth| relative.components().count() <= depth)
                    && matcher.as_ref().is_none_or(|m| m.is_match(relative))
            })
            .collect::<Vec<_>>();
        paths.extend(
            picker
                .get_dirs()
                .iter()
                .map(|item| PathBuf::from(item.relative_path(&*picker)))
                .filter(|path| path.starts_with(root_rel))
                .filter(|path| {
                    let relative = path.strip_prefix(root_rel).unwrap_or(path);
                    !relative.as_os_str().is_empty()
                        && max_depth.is_none_or(|depth| relative.components().count() <= depth)
                        && matcher.as_ref().is_none_or(|m| m.is_match(relative))
                }),
        );
        paths.sort();
        paths.dedup();
        let page = paths
            .iter()
            .skip(offset)
            .take(limit)
            .map(|path| {
                mount_path_to_uri(&self.mount_root, &self.mount_root.join(path))
                    .map(|u| u.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let next = (offset + page.len() < paths.len()).then(|| (offset + page.len()).to_string());
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
    ) -> Result<Value, String> {
        let picker = self.picker.lock().map_err(|_| "FFF index lock poisoned")?;
        let root = uri_to_mount_path(&self.mount_root, uri)?;
        let offset = parse_cursor(cursor)?;
        let include = include_glob
            .map(Glob::new)
            .transpose()
            .map_err(|e| e.to_string())?
            .map(|g| g.compile_matcher());
        let query = parse_grep_query(regex);
        let result = picker.grep(
            &query,
            &GrepSearchOptions {
                mode: GrepMode::Regex,
                page_limit: usize::MAX,
                before_context: context,
                after_context: context,
                ..Default::default()
            },
        );
        if let Some(error) = result.regex_fallback_error {
            return Err(format!("invalid regex: {error}"));
        }
        let mut matches = Vec::new();
        for found in result.matches {
            let file = result.files[found.file_index];
            let path = self.mount_root.join(file.relative_path(&*picker));
            if !path.starts_with(&root) {
                continue;
            }
            let relative = path.strip_prefix(&root).unwrap_or(&path);
            if include
                .as_ref()
                .is_some_and(|glob| !glob.is_match(relative))
            {
                continue;
            }
            matches.push(json!({
                "uri": mount_path_to_uri(&self.mount_root, &path)?.to_string(),
                "line": found.line_number,
                "column": found.col,
                "text": found.line_content,
                "before": found.context_before,
                "after": found.context_after,
            }));
        }
        let total = matches.len();
        let page: Vec<_> = matches.into_iter().skip(offset).take(limit).collect();
        let next = (offset + page.len() < total).then(|| (offset + page.len()).to_string());
        Ok(json!({"matches": page, "cursor": next}))
    }
}

fn build_picker(root: &Path) -> Result<FilePicker, String> {
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: root.to_string_lossy().into_owned(),
        mode: FFFMode::Ai,
        watch: true,
        enable_mmap_cache: false,
        enable_content_indexing: false,
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    picker.collect_files().map_err(|e| e.to_string())?;
    Ok(picker)
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize, String> {
    cursor
        .unwrap_or("0")
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
    let text = if scheme == "file" {
        format!("file:///{}", base.join("/"))
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
    fn one_fff_index_finds_and_greps_canonical_resources() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file/workspace/src/lib.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "pub fn indexed_symbol() {}\n").unwrap();
        std::fs::write(
            file.with_file_name("other.rs"),
            "pub fn indexed_symbol_too() {}\n",
        )
        .unwrap();
        let index = SearchEngine::new(temp.path()).unwrap();
        let root = ResourceUri::resolve("file:///workspace", Path::new("/")).unwrap();
        let found = index.find(&root, Some("**/*.rs"), None, None, 50).unwrap();
        assert_eq!(found["results"].as_array().unwrap().len(), 2);
        let matches = index
            .grep(&root, "indexed_symbol", Some("**/*.rs"), 0, None, 100)
            .unwrap();
        assert_eq!(matches["matches"][0]["uri"], "file:///workspace/src/lib.rs");
        assert_eq!(matches["matches"][0]["line"], 1);

        let first = index
            .grep(&root, "indexed_symbol", Some("**/*.rs"), 0, None, 1)
            .unwrap();
        assert_eq!(first["cursor"], "1");
        let second = index
            .grep(
                &root,
                "indexed_symbol",
                Some("**/*.rs"),
                0,
                first["cursor"].as_str(),
                1,
            )
            .unwrap();
        assert!(second["cursor"].is_null());
        assert_ne!(first["matches"][0]["uri"], second["matches"][0]["uri"]);
    }
}
