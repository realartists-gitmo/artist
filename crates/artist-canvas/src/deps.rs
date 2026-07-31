//! Packages the vendored set does not carry.
//!
//! The in-binary dependencies cover what canvases actually reach for, but not
//! everything. A canvas that genuinely needs `three` or `d3` declares it under
//! `[deps]` in its manifest, and the URL is fetched once and cached on disk.
//!
//! Proxying rather than pointing the import map straight at the CDN buys three
//! things: the canvas keeps working offline after the first load, the browser
//! never talks to a third party, and what a canvas pulled in is auditable on
//! disk instead of invisible in a page's network tab.

use std::path::{Path, PathBuf};

/// Where fetched packages live. Shared across projects — two canvases asking
/// for the same pinned URL should not each pay for it.
pub fn cache_root() -> PathBuf {
    std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|dir| dir.join("artist")))
        .unwrap_or_else(|| PathBuf::from(".artist"))
        .join("canvas-cache")
}

/// A URL's cache file name.
///
/// Content-addressed by URL rather than by package name, so two canvases
/// pinning different versions of the same package cannot collide.
fn cache_name(url: &str) -> String {
    // FNV-1a: this is a cache key, not a security boundary, and it avoids
    // pulling a hash crate in for one call site.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}.js")
}

#[derive(Debug, thiserror::Error)]
pub enum DepError {
    #[error("`{0}` is not declared in [deps] for this canvas")]
    NotDeclared(String),
    #[error("only https URLs may be declared as deps, got `{0}`")]
    NotHttps(String),
    #[error("could not fetch {url}: {source}")]
    Fetch {
        url: String,
        #[source]
        source: reqwest::Error,
    },
}

/// Fetch a declared dependency, from cache when possible.
///
/// `url` must come from the canvas's own manifest — never from the request —
/// so a page cannot turn this into an open proxy for arbitrary hosts.
pub async fn fetch(url: &str) -> Result<Vec<u8>, DepError> {
    if !url.starts_with("https://") {
        return Err(DepError::NotHttps(url.to_owned()));
    }

    let root = cache_root();
    let cached = root.join(cache_name(url));
    if let Ok(bytes) = std::fs::read(&cached) {
        return Ok(bytes);
    }

    let response = reqwest::get(url)
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| DepError::Fetch {
            url: url.to_owned(),
            source,
        })?;
    let bytes = response
        .bytes()
        .await
        .map_err(|source| DepError::Fetch {
            url: url.to_owned(),
            source,
        })?
        .to_vec();

    // Cache write is best-effort: a canvas must still work on a read-only or
    // full disk, just without the offline benefit.
    let _ = std::fs::create_dir_all(&root);
    let _ = write_atomically(&cached, &bytes);

    Ok(bytes)
}

fn write_atomically(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = target.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_names_are_stable_and_version_specific() {
        let one = "https://esm.sh/three@0.170";
        let two = "https://esm.sh/three@0.171";

        assert_eq!(cache_name(one), cache_name(one), "not stable");
        assert_ne!(
            cache_name(one),
            cache_name(two),
            "two pinned versions would share a cache entry"
        );
        assert!(cache_name(one).ends_with(".js"));
    }

    /// A cache file name becomes a path. It must never contain a separator or
    /// anything else that could escape the cache directory.
    #[test]
    fn cache_names_are_safe_path_segments() {
        for url in [
            "https://esm.sh/../../etc/passwd",
            "https://example.com/a/b/c?x=1&y=2#frag",
        ] {
            let name = cache_name(url);
            assert!(!name.contains('/'), "{name}");
            assert!(!name.contains('.') || name.ends_with(".js"), "{name}");
            assert_eq!(Path::new(&name).components().count(), 1, "{name}");
        }
    }

    #[tokio::test]
    async fn plain_http_is_refused() {
        let error = fetch("http://example.com/x.js").await.expect_err("refused");
        assert!(matches!(error, DepError::NotHttps(_)));
    }

    #[tokio::test]
    async fn a_relative_or_file_url_is_refused() {
        for url in ["./local.js", "file:///etc/passwd", "/@vendor/react.js"] {
            assert!(
                matches!(fetch(url).await, Err(DepError::NotHttps(_))),
                "{url} should be refused"
            );
        }
    }
}
