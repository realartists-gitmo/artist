use std::fmt;
use std::str::FromStr;

/// A canonical resource address. The kernel owns URI parsing and normalization;
/// providers own the meaning of the normalized path after routing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceUri {
    scheme: String,
    authority: String,
    path: String,
    query: Option<String>,
    // Fragments are preserved verbatim at the kernel boundary. Their position
    // grammar is intentionally not implemented here yet.
    //
    // TODO(teca): add the smallest adapter that attempts a canonical Teca
    // address first, with decimal line numbers as the explicitly temporary
    // compatibility fallback. Do not introduce an Artist-owned anchor format.
    fragment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UriError {
    MissingScheme,
    InvalidScheme,
    InvalidPath,
}

impl fmt::Display for UriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingScheme => write!(f, "resource URI is missing a scheme"),
            Self::InvalidScheme => write!(f, "resource URI has an invalid scheme"),
            Self::InvalidPath => write!(f, "resource URI contains an invalid path component"),
        }
    }
}

impl std::error::Error for UriError {}

impl ResourceUri {
    /// Construct a `files://` URI from an OS-style path. Relative paths are
    /// rooted at the files namespace root; `.` is normalized and traversal
    /// above that root is rejected.
    pub fn from_path(path: impl AsRef<str>) -> Result<Self, UriError> {
        let path = path.as_ref();
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Self::new("files", "", path)
    }

    pub fn new(
        scheme: impl Into<String>,
        authority: impl Into<String>,
        path: impl AsRef<str>,
    ) -> Result<Self, UriError> {
        let scheme = scheme.into().to_ascii_lowercase();
        validate_scheme(&scheme)?;
        let path = normalize_path(path.as_ref())?;
        Ok(Self {
            scheme,
            authority: authority.into(),
            path,
            query: None,
            fragment: None,
        })
    }

    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    pub fn with_fragment(mut self, fragment: impl Into<String>) -> Self {
        self.fragment = Some(fragment.into());
        self
    }

    /// Return the resource address without its position fragment.
    ///
    /// Fragments identify a position within a resource and are not part of
    /// provider routing or storage lookup. The future Teca adapter will parse
    /// them at the verb boundary; until then, the raw fragment is preserved
    /// by this type and ignored by resource operations.
    pub fn without_fragment(&self) -> Self {
        let mut uri = self.clone();
        uri.fragment = None;
        uri
    }

    pub fn root(scheme: impl Into<String>) -> Result<Self, UriError> {
        Self::new(scheme, "", "/")
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }
    pub fn authority(&self) -> &str {
        &self.authority
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }
    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.path.split('/').filter(|segment| !segment.is_empty())
    }

    pub fn is_root(&self) -> bool {
        self.path == "/"
    }

    pub fn child(&self, name: &str) -> Result<Self, UriError> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err(UriError::InvalidPath);
        }
        let path = if self.is_root() {
            format!("/{name}")
        } else {
            format!("{}/{name}", self.path.trim_end_matches('/'))
        };
        Self::new(self.scheme.clone(), self.authority.clone(), path)
    }

    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        let path = self.path.rsplit_once('/').map(|(parent, _)| {
            if parent.is_empty() {
                "/".to_string()
            } else {
                parent.to_string()
            }
        })?;
        Self::new(self.scheme.clone(), self.authority.clone(), path).ok()
    }

    pub fn starts_with(&self, prefix: &Self) -> bool {
        if self.scheme != prefix.scheme || self.authority != prefix.authority {
            return false;
        }
        if prefix.is_root() {
            return true;
        }
        self.path == prefix.path
            || self
                .path
                .starts_with(&format!("{}/", prefix.path.trim_end_matches('/')))
    }
}

impl FromStr for ResourceUri {
    type Err = UriError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (base, fragment) = match value.split_once('#') {
            Some((base, fragment)) => (base, Some(fragment.to_string())),
            None => (value, None),
        };
        let (base, query) = match base.split_once('?') {
            Some((base, query)) => (base, Some(query.to_string())),
            None => (base, None),
        };

        let Some((scheme, rest)) = base.split_once("://") else {
            let mut uri = Self::from_path(base)?;
            uri.query = query;
            uri.fragment = fragment;
            return Ok(uri);
        };
        let uri = if rest.starts_with('/') {
            Self::new(scheme, "", rest)?
        } else if let Some((authority, suffix)) = rest.split_once('/') {
            Self::new(scheme, authority, format!("/{suffix}"))?
        } else {
            Self::new(scheme, rest, "/")?
        };
        Ok(Self {
            query,
            fragment,
            ..uri
        })
    }
}

impl fmt::Display for ResourceUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}{}", self.scheme, self.authority, self.path)?;
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }
        if let Some(fragment) = &self.fragment {
            write!(f, "#{fragment}")?;
        }
        Ok(())
    }
}

fn validate_scheme(scheme: &str) -> Result<(), UriError> {
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return Err(UriError::InvalidScheme);
    };
    if !first.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return Err(UriError::InvalidScheme);
    }
    Ok(())
}

fn normalize_path(path: &str) -> Result<String, UriError> {
    if path.is_empty() {
        return Ok("/".to_string());
    }
    if !path.starts_with('/') || path.contains('\\') {
        return Err(UriError::InvalidPath);
    }
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(UriError::InvalidPath),
            value => segments.push(value),
        }
    }
    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes() {
        let uri: ResourceUri = "FILES:///src/main.rs".parse().unwrap();
        assert_eq!(uri.scheme(), "files");
        assert_eq!(uri.path(), "/src/main.rs");
        assert_eq!(uri.to_string(), "files:///src/main.rs");
        assert_eq!(uri.parent().unwrap().to_string(), "files:///src");
    }

    #[test]
    fn authorities_are_preserved() {
        let uri: ResourceUri = "repo://owner/pr/1".parse().unwrap();
        assert_eq!(uri.authority(), "owner");
        assert_eq!(uri.path(), "/pr/1");
    }

    #[test]
    fn bare_paths_default_to_files_namespace() {
        let relative: ResourceUri = "src/./main.rs?kind#body".parse().unwrap();
        assert_eq!(relative.to_string(), "files:///src/main.rs?kind#body");

        let absolute: ResourceUri = "/workspace/main.rs".parse().unwrap();
        assert_eq!(absolute.to_string(), "files:///workspace/main.rs");
        assert!("../secret".parse::<ResourceUri>().is_err());
    }

    #[test]
    fn queries_and_fragments_are_preserved() {
        let uri: ResourceUri = "files:///src/main.rs?kind#body".parse().unwrap();
        assert_eq!(uri.path(), "/src/main.rs");
        assert_eq!(uri.query(), Some("kind"));
        assert_eq!(uri.fragment(), Some("body"));
        assert_eq!(uri.to_string(), "files:///src/main.rs?kind#body");
    }

    #[test]
    fn path_operations_do_not_turn_selectors_into_path_components() {
        let uri: ResourceUri = "files:///src/main.rs?kind#body".parse().unwrap();
        assert_eq!(uri.segments().collect::<Vec<_>>(), vec!["src", "main.rs"]);
        assert_eq!(uri.parent().unwrap().to_string(), "files:///src");
        assert_eq!(
            uri.child("next.rs").unwrap().to_string(),
            "files:///src/main.rs/next.rs"
        );
    }

    #[test]
    fn traversal_is_rejected() {
        assert!("files:///../secret".parse::<ResourceUri>().is_err());
        assert!("files:///a/../../secret".parse::<ResourceUri>().is_err());
    }
}
