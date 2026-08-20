use std::fmt;
use std::str::FromStr;

/// A canonical resource address. The kernel owns URI parsing and normalization;
/// providers own the meaning of the normalized path after routing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceUri {
    scheme: String,
    authority: String,
    path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UriError {
    MissingScheme,
    InvalidScheme,
    QueryOrFragment,
    InvalidPath,
}

impl fmt::Display for UriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingScheme => write!(f, "resource URI is missing a scheme"),
            Self::InvalidScheme => write!(f, "resource URI has an invalid scheme"),
            Self::QueryOrFragment => write!(f, "resource URI cannot contain a query or fragment"),
            Self::InvalidPath => write!(f, "resource URI contains an invalid path component"),
        }
    }
}

impl std::error::Error for UriError {}

impl ResourceUri {
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
        })
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
        let (scheme, rest) = value.split_once("://").ok_or(UriError::MissingScheme)?;
        if rest.contains('?') || rest.contains('#') {
            return Err(UriError::QueryOrFragment);
        }
        if rest.starts_with('/') {
            return Self::new(scheme, "", rest);
        }
        if let Some((authority, suffix)) = rest.split_once('/') {
            return Self::new(scheme, authority, format!("/{suffix}"));
        }
        Self::new(scheme, rest, "/")
    }
}

impl fmt::Display for ResourceUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}{}", self.scheme, self.authority, self.path)
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
    fn traversal_is_rejected() {
        assert!("files:///../secret".parse::<ResourceUri>().is_err());
        assert!("files:///a/../../secret".parse::<ResourceUri>().is_err());
    }
}
