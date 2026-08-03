use url::Url;

/// Canvas URLs carry the unguessable server key in `/c/<key>/<slug>/` and are
/// only valid on a loopback listener owned by the session host.
pub(crate) fn is_authenticated_local_canvas_origin(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "http" || url.port().is_none() {
        return false;
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    let segments = url
        .path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    loopback
        && segments.len() >= 3
        && segments[0] == "c"
        && segments[1].len() >= 16
        && !segments[2].is_empty()
}

pub(crate) fn same_origin(allowed: &str, candidate: &str) -> bool {
    let (Ok(allowed), Ok(candidate)) = (Url::parse(allowed), Url::parse(candidate)) else {
        return false;
    };
    allowed.scheme() == candidate.scheme()
        && allowed.host_str() == candidate.host_str()
        && allowed.port_or_known_default() == candidate.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_keyed_loopback_canvas_urls_are_accepted() {
        assert!(is_authenticated_local_canvas_origin(
            "http://127.0.0.1:43111/c/0123456789abcdef/demo/"
        ));
        assert!(is_authenticated_local_canvas_origin(
            "http://[::1]:43111/c/0123456789abcdef/demo/"
        ));
        assert!(!is_authenticated_local_canvas_origin(
            "https://example.com:43111/c/0123456789abcdef/demo/"
        ));
        assert!(!is_authenticated_local_canvas_origin(
            "http://127.0.0.1:43111/demo/"
        ));
    }

    #[test]
    fn navigation_is_confined_to_the_endpoint_origin() {
        let allowed = "http://127.0.0.1:43111/c/0123456789abcdef/demo/";
        assert!(same_origin(allowed, "http://127.0.0.1:43111/@artist/ui.js"));
        assert!(!same_origin(allowed, "http://127.0.0.1:43112/elsewhere"));
        assert!(!same_origin(allowed, "https://example.com/"));
    }
}
