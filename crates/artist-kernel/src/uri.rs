use crate::KernelError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, path::Path, str::FromStr};
use url::Url;

/// A canonical resource URI.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResourceUri(Url);

impl ResourceUri {
    pub fn parse(value: &str) -> Result<Self, KernelError> {
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
        if url.scheme().is_empty() {
            return Err(KernelError::InvalidUri {
                message: format!("{value:?}: URI has no scheme"),
            });
        }
        if url.query().is_some() {
            return Err(KernelError::InvalidUri {
                message: format!("{value:?}: query components are not supported"),
            });
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
    fn accepts_fragments_but_rejects_query_components() {
        assert!(ResourceUri::parse("mem://notes/todo?all=true").is_err());
        let uri = ResourceUri::parse("mem://notes/todo#anchor").unwrap();
        assert_eq!(uri.fragment(), Some("anchor"));
        assert_eq!(uri.to_string(), "mem://notes/todo#anchor");
    }

    #[test]
    fn canonicalizes_bare_paths_to_file_uris() {
        let uri = ResourceUri::parse("src/main.rs").unwrap();
        assert_eq!(uri.scheme(), "file");
        assert!(uri.path().ends_with("/src/main.rs"));
        assert!(uri.to_string().starts_with("file://"));
    }
}
