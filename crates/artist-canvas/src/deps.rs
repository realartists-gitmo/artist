//! Packages the vendored set does not carry.
//!
//! The in-binary dependencies cover what canvases actually reach for, but not
//! everything. A canvas that genuinely needs `three` or `d3` declares it under
//! `[deps]` in its manifest, and the browser loads it from there.
//!
//! The import map points at the CDN directly. Proxying was tried and silently
//! broke every declared dep: a CDN entry point re-exports from a root-relative
//! path, which the browser resolves against whichever origin served it, so
//! behind the proxy it pointed back at the canvas server and 404d.
//!
//! # Why this is restrictive
//!
//! `canvas.toml` is model-written, and a declared URL is one the user's browser
//! will load. An unchecked one is an outbound-request primitive handed to the
//! model: `https://10.0.0.1/admin` is a valid https URL. So the host must be
//! one of a short list — and since nothing is fetched server-side any more,
//! [`check`] at import-map construction is the only thing enforcing it.

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

#[derive(Debug, thiserror::Error)]
pub enum DepError {
    #[error("only https URLs may be declared as deps, got `{0}`")]
    NotHttps(String),
    #[error(
        "`{host}` is not an allowed dependency host — use one of: {}",
        ALLOWED_HOSTS.join(", ")
    )]
    HostNotAllowed { host: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_and_non_urls_are_refused() {
        for url in [
            "http://esm.sh/x.js",
            "./local.js",
            "file:///etc/passwd",
            "/@vendor/react.js",
        ] {
            assert!(
                matches!(check(url), Err(DepError::NotHttps(_))),
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
