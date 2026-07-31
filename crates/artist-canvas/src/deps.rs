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
//!
//! # Why this is restrictive
//!
//! `canvas.toml` is model-written, and this fetches from the user's host. An
//! unrestricted fetcher is an outbound-request primitive handed to the model:
//! `https://10.0.0.1/admin` is a valid https URL. So the host must be one of a
//! short list, the response is bounded, and the request times out.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest, Sha256};

/// Hosts a canvas may pull a module from.
///
/// An allowlist rather than a private-range blocklist: blocklists are defeated
/// by DNS, redirects, and IPv6 mapping, and the legitimate use of `[deps]` is
/// pulling a public package from a public CDN.
pub const ALLOWED_HOSTS: &[&str] = &[
    "esm.sh",
    "cdn.jsdelivr.net",
    "unpkg.com",
    "cdn.skypack.dev",
    "esm.run",
    "ga.jspm.io",
];

/// A dependency is a module, not a dataset.
const MAX_BYTES: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

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
/// SHA-256 rather than a fast non-cryptographic hash. The cache is shared
/// across every project on the machine, so its key crosses a trust boundary:
/// with a preimageable hash, a canvas in one project could choose bytes that
/// land on the entry another project's `react` reads. That makes it a security
/// boundary whatever the intent was.
fn cache_name(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::with_capacity(64 + 3);
    for byte in digest {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push_str(".js");
    name
}

#[derive(Debug, thiserror::Error)]
pub enum DepError {
    #[error("only https URLs may be declared as deps, got `{0}`")]
    NotHttps(String),
    #[error(
        "`{host}` is not an allowed dependency host — use one of: {}",
        ALLOWED_HOSTS.join(", ")
    )]
    HostNotAllowed { host: String },
    #[error("{url} returned {size} bytes; the limit is {MAX_BYTES}")]
    TooLarge { url: String, size: usize },
    #[error("could not fetch {url}: {source}")]
    Fetch {
        url: String,
        #[source]
        source: reqwest::Error,
    },
}

/// The host part of an https URL, lowercased and without a port.
fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    // Credentials would let `https://esm.sh@evil.example/` read as allowed.
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Is this URL one a canvas may declare?
pub fn check(url: &str) -> Result<(), DepError> {
    if !url.starts_with("https://") {
        return Err(DepError::NotHttps(url.to_owned()));
    }
    let host = host_of(url).ok_or_else(|| DepError::NotHttps(url.to_owned()))?;
    if !ALLOWED_HOSTS.iter().any(|allowed| host == *allowed) {
        return Err(DepError::HostNotAllowed { host });
    }
    Ok(())
}

/// Fetch a declared dependency, from cache when possible.
///
/// `url` must come from the canvas's own manifest — never from the request —
/// so a page cannot turn this into an open proxy.
pub async fn fetch(url: &str) -> Result<Vec<u8>, DepError> {
    check(url)?;

    let root = cache_root();
    let cached = root.join(cache_name(url));
    if let Ok(bytes) = std::fs::read(&cached) {
        return Ok(bytes);
    }

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        // A redirect is how an allowed host would otherwise reach a disallowed
        // one, so the allowlist has to hold for the whole chain.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            match check(attempt.url().as_str()) {
                Ok(()) if attempt.previous().len() < 5 => attempt.follow(),
                _ => attempt.stop(),
            }
        }))
        .build()
        .map_err(|source| DepError::Fetch {
            url: url.to_owned(),
            source,
        })?;

    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| DepError::Fetch {
            url: url.to_owned(),
            source,
        })?;

    if let Some(length) = response.content_length()
        && length as usize > MAX_BYTES
    {
        return Err(DepError::TooLarge {
            url: url.to_owned(),
            size: length as usize,
        });
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|source| DepError::Fetch {
            url: url.to_owned(),
            source,
        })?
        .to_vec();

    // Checked again: a server may omit content-length or lie about it.
    if bytes.len() > MAX_BYTES {
        return Err(DepError::TooLarge {
            url: url.to_owned(),
            size: bytes.len(),
        });
    }

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
        assert_ne!(cache_name(one), cache_name(two));
        assert!(cache_name(one).ends_with(".js"));
        // 64 hex characters plus the extension: a full SHA-256, not a truncation.
        assert_eq!(cache_name(one).len(), 64 + 3);
    }

    /// The cache is shared across projects, so its key crosses a trust
    /// boundary. A fast hash was preimageable in a few bytes.
    #[test]
    fn cache_names_are_safe_path_segments() {
        for url in [
            "https://esm.sh/../../etc/passwd",
            "https://example.com/a/b/c?x=1&y=2#frag",
        ] {
            let name = cache_name(url);
            assert!(!name.contains('/'), "{name}");
            assert_eq!(Path::new(&name).components().count(), 1, "{name}");
        }
    }

    #[tokio::test]
    async fn plain_http_and_non_urls_are_refused() {
        for url in ["http://esm.sh/x.js", "./local.js", "file:///etc/passwd", "/@vendor/react.js"] {
            assert!(
                matches!(fetch(url).await, Err(DepError::NotHttps(_))),
                "{url} should be refused"
            );
        }
    }

    /// `canvas.toml` is model-written and this fetches from the user's host, so
    /// an unrestricted fetcher is an outbound-request primitive.
    #[test]
    fn only_known_cdns_are_reachable() {
        assert!(check("https://esm.sh/three").is_ok());
        assert!(check("https://cdn.jsdelivr.net/npm/three").is_ok());

        for blocked in [
            "https://10.0.0.1/admin",
            "https://192.168.1.1/",
            "https://localhost:8443/",
            "https://[::1]/",
            "https://169.254.169.254/latest/meta-data/",
            "https://evil.example/payload.js",
        ] {
            assert!(
                matches!(check(blocked), Err(DepError::HostNotAllowed { .. })),
                "{blocked} was allowed"
            );
        }
    }

    /// `https://esm.sh@evil.example/` has an authority of `esm.sh@evil.example`
    /// and a host of `evil.example`; a naive prefix check reads it as allowed.
    #[test]
    fn credentials_cannot_disguise_the_host() {
        assert!(check("https://esm.sh@evil.example/x.js").is_err());
        assert!(check("https://user:pass@evil.example/x.js").is_err());
    }

    #[test]
    fn a_port_does_not_defeat_the_allowlist() {
        assert_eq!(host_of("https://esm.sh:443/x").as_deref(), Some("esm.sh"));
        assert!(check("https://evil.example:443/x").is_err());
    }

    #[test]
    fn host_matching_is_exact_not_a_suffix() {
        // `notesm.sh` and `esm.sh.evil.example` both contain the allowed host.
        assert!(check("https://notesm.sh/x").is_err());
        assert!(check("https://esm.sh.evil.example/x").is_err());
        assert!(check("https://ESM.SH/x").is_ok(), "case should not matter");
    }
}
