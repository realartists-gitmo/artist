use std::{
    fmt,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use url::Url;

const PROJECTION: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'?');

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResourceUri(Url);

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum UriError {
    #[error("resource URI fragments are not supported")]
    Fragment,
    #[error("invalid resource URI: {0}")]
    Invalid(String),
    #[error("resource projection is not valid UTF-8")]
    ProjectionEncoding,
}

impl ResourceUri {
    pub fn resolve(input: &str, working_directory: &Path) -> Result<Self, UriError> {
        if input.contains('#') {
            return Err(UriError::Fragment);
        }
        let mut url = if has_scheme(input) {
            Url::parse(input).map_err(|e| UriError::Invalid(e.to_string()))?
        } else {
            let (path, projection) = input
                .split_once('?')
                .map_or((input, None), |(a, b)| (a, Some(b)));
            let path = normalize(if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                working_directory.join(path)
            });
            let mut url = Url::from_file_path(path).map_err(|_| {
                UriError::Invalid("filesystem path cannot be represented as a URL".into())
            })?;
            if let Some(projection) = projection {
                let encoded = encode_projection_text(projection)?;
                url.set_query(Some(&encoded));
            }
            url
        };
        if url.fragment().is_some() {
            return Err(UriError::Fragment);
        }
        url.set_fragment(None);
        validate_projection(&url)?;
        if let Some(query) = url.query() {
            let canonical = query
                .split('/')
                .map(|segment| {
                    percent_decode_str(segment)
                        .decode_utf8()
                        .map(|value| utf8_percent_encode(&value, PROJECTION).to_string())
                        .map_err(|_| UriError::ProjectionEncoding)
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            url.set_query(Some(&canonical));
        }
        Ok(Self(url))
    }

    pub fn from_url(url: Url) -> Result<Self, UriError> {
        Self::resolve(url.as_str(), Path::new("/"))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn base(&self) -> Self {
        let mut url = self.0.clone();
        url.set_query(None);
        Self(url)
    }

    pub fn projection_segments(&self) -> Vec<String> {
        self.0
            .query()
            .map(|query| {
                query
                    .split('/')
                    .map(|segment| {
                        percent_decode_str(segment)
                            .decode_utf8()
                            .expect("validated URI")
                            .into_owned()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn descend_projection(&self, segment: &str) -> Result<Self, UriError> {
        if segment.is_empty() || segment.contains('/') {
            return Err(UriError::Invalid(
                "a projection descendant must be one non-empty segment".into(),
            ));
        }
        let mut segments = self.projection_segments();
        segments.push(segment.to_owned());
        let query = segments
            .iter()
            .map(|s| utf8_percent_encode(s, PROJECTION).to_string())
            .collect::<Vec<_>>()
            .join("/");
        let mut url = self.0.clone();
        url.set_query(Some(&query));
        Ok(Self(url))
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        (self.0.scheme() == "file")
            .then(|| self.0.to_file_path().ok())
            .flatten()
    }
}

fn has_scheme(value: &str) -> bool {
    value.find(':').is_some_and(|i| {
        i > 0
            && value[..i].bytes().enumerate().all(|(n, b)| {
                b.is_ascii_alphabetic()
                    || n > 0 && (b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'))
            })
    })
}

fn normalize(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn encode_projection_text(value: &str) -> Result<String, UriError> {
    if value.is_empty() {
        return Err(UriError::Invalid("projection cannot be empty".into()));
    }
    value
        .split('/')
        .map(|segment| {
            if segment.is_empty() {
                Err(UriError::Invalid(
                    "projection segments cannot be empty".into(),
                ))
            } else {
                Ok(utf8_percent_encode(segment, PROJECTION).to_string())
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join("/"))
}

fn validate_projection(url: &Url) -> Result<(), UriError> {
    if let Some(query) = url.query() {
        if query.is_empty() || query.split('/').any(str::is_empty) {
            return Err(UriError::Invalid(
                "projection segments cannot be empty".into(),
            ));
        }
        for segment in query.split('/') {
            percent_decode_str(segment)
                .decode_utf8()
                .map_err(|_| UriError::ProjectionEncoding)?;
        }
    }
    Ok(())
}

impl fmt::Display for ResourceUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl FromStr for ResourceUri {
    type Err = UriError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::resolve(
            s,
            &std::env::current_dir().map_err(|e| UriError::Invalid(e.to_string()))?,
        )
    }
}
impl Serialize for ResourceUri {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0.as_str())
    }
}
impl<'de> Deserialize<'de> for ResourceUri {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_shorthand_and_projection_round_trip() {
        let uri = ResourceUri::resolve("src/a b.rs?symbols/foo%bar", Path::new("/work")).unwrap();
        assert_eq!(
            uri.to_string(),
            "file:///work/src/a%20b.rs?symbols/foo%25bar"
        );
        assert_eq!(uri.projection_segments(), ["symbols", "foo%bar"]);
        assert_eq!(uri.base().to_string(), "file:///work/src/a%20b.rs");
        assert_eq!(
            uri.descend_projection("callers/x").unwrap_err(),
            UriError::Invalid("a projection descendant must be one non-empty segment".into())
        );
    }

    #[test]
    fn descent_encodes_literal_delimiters() {
        let uri = ResourceUri::resolve("src/lib.rs?symbols", Path::new("/work"))
            .unwrap()
            .descend_projection("a?b")
            .unwrap();
        assert_eq!(uri.to_string(), "file:///work/src/lib.rs?symbols/a%3Fb");
        assert_eq!(uri.projection_segments(), ["symbols", "a?b"]);
    }

    #[test]
    fn rejects_fragments() {
        assert_eq!(
            ResourceUri::resolve("file:///x#y", Path::new("/")).unwrap_err(),
            UriError::Fragment
        );
        assert_eq!(
            ResourceUri::resolve("src/lib.rs#symbol", Path::new("/work")).unwrap_err(),
            UriError::Fragment
        );
    }

    #[test]
    fn canonicalizes_projection_literals_in_full_urls() {
        let uri = ResourceUri::resolve("file:///work/a.rs?symbols/a?b", Path::new("/")).unwrap();
        assert_eq!(uri.to_string(), "file:///work/a.rs?symbols/a%3Fb");
        assert_eq!(uri.projection_segments(), ["symbols", "a?b"]);
    }
}
