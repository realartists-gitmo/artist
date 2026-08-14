use crate::KernelError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, path::Path, str::FromStr};
use url::Url;

/// A canonical resource URI.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResourceUri(Url);

impl ResourceUri {
    pub fn parse(value: &str) -> Result<Self, KernelError> {
        let directory_hint = value.ends_with('/') || value.ends_with('\\');
        let url = match Url::parse(value) {
            Ok(url) => url,
            Err(_) => {
                let path = Path::new(value);
                let path = if path.is_absolute() {
                    path.to_owned()
                } else {
                    std::env::current_dir()
                        .map_err(|error| KernelError::InvalidUri {
                            message: format!(
                                "{value:?}: cannot resolve current directory: {error}"
                            ),
                        })?
                        .join(path)
                };
                Url::from_file_path(path).map_err(|_| KernelError::InvalidUri {
                    message: format!("{value:?}: not a valid URI or filesystem path"),
                })?
            }
        };
        let mut url = url;
        if directory_hint && url.scheme() == "file" && !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        if url.scheme().is_empty() {
            return Err(KernelError::InvalidUri {
                message: format!("{value:?}: URI has no scheme"),
            });
        }
        if let Some(fragment) = url.fragment()
            && fragment.contains('?')
        {
            return Err(KernelError::InvalidUri {
                message: format!("{value:?}: query syntax must precede the fragment"),
            });
        }
        if let Some(query) = url.query() {
            validate_query(query).map_err(|message| KernelError::InvalidUri { message })?;
        }
        Ok(Self(url))
    }

    pub fn scheme(&self) -> &str {
        self.0.scheme()
    }

    pub fn path(&self) -> &str {
        self.0.path()
    }

    pub fn fragment(&self) -> Option<&str> {
        self.0.fragment()
    }

    pub fn query(&self) -> Option<&str> {
        self.0.query()
    }

    pub fn without_fragment(&self) -> Self {
        let mut url = self.0.clone();
        url.set_fragment(None);
        Self(url)
    }

    pub fn with_fragment(&self, fragment: impl AsRef<str>) -> Self {
        let mut url = self.0.clone();
        url.set_fragment(Some(fragment.as_ref()));
        Self(url)
    }

    /// Replace the raw query without normalizing its ordering or collapsing
    /// bare keys. Query text is part of Artist resource-view identity.
    pub fn with_query(&self, query: impl AsRef<str>) -> Self {
        let mut url = self.0.clone();
        url.set_query(Some(query.as_ref()));
        Self(url)
    }

    pub fn query_items(&self) -> Result<Vec<QueryItem>, KernelError> {
        let Some(query) = self.query() else {
            return Ok(Vec::new());
        };
        validate_query(query).map_err(|message| KernelError::InvalidUri { message })?;
        Ok(query
            .split('&')
            .map(|item| {
                let (raw_key, raw_value) = item
                    .split_once('=')
                    .map_or((item, None), |(key, value)| (key, Some(value)));
                QueryItem {
                    key: decode_query_component(raw_key),
                    value: raw_value.map(decode_query_component),
                }
            })
            .collect())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryItem {
    pub key: String,
    pub value: Option<String>,
}

fn decode_query_component(value: &str) -> String {
    url::form_urlencoded::parse(value.as_bytes())
        .next()
        .map(|(decoded, _)| decoded.into_owned())
        .unwrap_or_default()
}

fn validate_query(query: &str) -> Result<(), String> {
    for item in query.split('&') {
        let key = item.split_once('=').map_or(item, |(key, _)| key);
        let decoded = decode_query_component(key);
        let mut chars = decoded.chars();
        let Some(first) = chars.next() else {
            return Err("query keys must not be empty".to_owned());
        };
        if !(first.is_ascii_alphabetic() || first == '_')
            || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
        {
            return Err(format!("invalid query key {decoded:?}"));
        }
    }
    Ok(())
}

impl AsRef<Url> for ResourceUri {
    fn as_ref(&self) -> &Url {
        &self.0
    }
}

impl fmt::Display for ResourceUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ResourceUri {
    type Err = KernelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ResourceUri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for ResourceUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_serializes_conventional_resource_uris() {
        let uri = ResourceUri::parse("mem://notes/todo").unwrap();
        assert_eq!(uri.scheme(), "mem");
        assert_eq!(uri.path(), "/todo");
        assert_eq!(uri.to_string(), "mem://notes/todo");
    }

    #[test]
    fn accepts_ordered_query_components_and_fragments() {
        let queried = ResourceUri::parse("mem://notes/todo?flag&label=a&label=b#anchor").unwrap();
        assert_eq!(
            queried.query_items().unwrap(),
            vec![
                QueryItem {
                    key: "flag".to_owned(),
                    value: None
                },
                QueryItem {
                    key: "label".to_owned(),
                    value: Some("a".to_owned())
                },
                QueryItem {
                    key: "label".to_owned(),
                    value: Some("b".to_owned())
                },
            ]
        );
        assert_eq!(
            queried.without_fragment().to_string(),
            "mem://notes/todo?flag&label=a&label=b"
        );
        let uri = ResourceUri::parse("mem://notes/todo#anchor").unwrap();
        assert_eq!(uri.fragment(), Some("anchor"));
        assert_eq!(uri.to_string(), "mem://notes/todo#anchor");
        assert!(ResourceUri::parse("mem://notes/todo#anchor?all=true").is_err());
    }

    #[test]
    fn canonicalizes_bare_paths_to_file_uris() {
        let uri = ResourceUri::parse("src/main.rs").unwrap();
        assert_eq!(uri.scheme(), "file");
        assert!(uri.path().ends_with("/src/main.rs"));
        assert!(uri.to_string().starts_with("file://"));
    }

    #[test]
    fn replacing_query_preserves_raw_order_and_bare_key_shape() {
        let base = ResourceUri::parse("file:///src/main.rs").unwrap();
        let viewed = base.with_query("flag&label=a&label=b");
        assert_eq!(
            viewed.to_string(),
            "file:///src/main.rs?flag&label=a&label=b"
        );
        assert_eq!(
            viewed.query_items().unwrap()[0],
            QueryItem {
                key: "flag".to_owned(),
                value: None
            }
        );
    }
}
