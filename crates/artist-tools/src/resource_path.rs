//! The address grammar shared by real files and Artist virtual resources.
//!
//! Parsing is deliberately separate from resolution: an address can be
//! well-formed before a particular environment has a resolver for its scheme.
//! That keeps callers from accidentally treating `agent://goethe` as a funny
//! relative filesystem path.
//!
//! Production extension roots have stable shapes: `eval://<session>`,
//! `debug://<session>`, `forge://<provider>/<repository>/...`,
//! `code://<workspace>/<projection>/...`, `relation://<source>/<edge>/...`,
//! and `rules://<package>/<rule>/...`. A root remains a typed path even before
//! its specialized resolver is installed.

use std::fmt;

/// Render a non-file text resource as TECA-anchored logical lines. The caller
/// owns the resource revision; this helper only defines the common text view.
pub fn render_anchored_virtual_text(text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return "[empty]".into();
    }
    hashline_tools::virtual_line_anchors(&lines, "virtual/terminal/logical-line")
        .into_iter()
        .zip(lines)
        .map(|(anchor, line)| format!("{anchor}: {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Revision token for a virtual text resource's canonical bytes.
pub fn virtual_text_revision(text: &str) -> String {
    hashline_tools::content_hash(text.as_bytes())
}

/// The virtual roots defined by the Artist production contract.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ResourceScheme {
    Agent,
    Bash,
    Ask,
    Canvas,
    Computer,
    /// One-shot native executable snapshots. These share the terminal backend
    /// with `bash://` but have their own canonical public identity.
    Process,
    Eval,
    Debug,
    Forge,
    Code,
    Relation,
    Rules,
    Artifact,
    Dict,
    Memory,
    Profile,
    Skill,
    Tools,
}

impl ResourceScheme {
    pub const ALL: [Self; 18] = [
        Self::Agent,
        Self::Bash,
        Self::Ask,
        Self::Canvas,
        Self::Computer,
        Self::Process,
        Self::Eval,
        Self::Debug,
        Self::Forge,
        Self::Code,
        Self::Relation,
        Self::Rules,
        Self::Artifact,
        Self::Dict,
        Self::Memory,
        Self::Profile,
        Self::Skill,
        Self::Tools,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Bash => "bash",
            Self::Ask => "ask",
            Self::Canvas => "canvas",
            Self::Computer => "computer",
            Self::Process => "process",
            Self::Eval => "eval",
            Self::Debug => "debug",
            Self::Forge => "forge",
            Self::Code => "code",
            Self::Relation => "relation",
            Self::Rules => "rules",
            Self::Artifact => "artifact",
            Self::Dict => "dict",
            Self::Memory => "memory",
            Self::Profile => "profile",
            Self::Skill => "skill",
            Self::Tools => "tools",
        }
    }
}

impl fmt::Display for ResourceScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for ResourceScheme {
    type Error = ResourcePathError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "agent" => Ok(Self::Agent),
            "bash" => Ok(Self::Bash),
            "ask" => Ok(Self::Ask),
            "canvas" => Ok(Self::Canvas),
            "computer" => Ok(Self::Computer),
            "process" => Ok(Self::Process),
            "eval" => Ok(Self::Eval),
            "debug" => Ok(Self::Debug),
            "forge" => Ok(Self::Forge),
            "code" => Ok(Self::Code),
            "relation" => Ok(Self::Relation),
            "rules" => Ok(Self::Rules),
            "artifact" => Ok(Self::Artifact),
            "dict" => Ok(Self::Dict),
            "memory" => Ok(Self::Memory),
            "profile" => Ok(Self::Profile),
            "skill" => Ok(Self::Skill),
            "tools" => Ok(Self::Tools),
            _ => Err(ResourcePathError::UnknownScheme(value.to_owned())),
        }
    }
}

/// A real filesystem input or a normalized virtual noun-space address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourcePath {
    Real(String),
    Virtual {
        scheme: ResourceScheme,
        /// Empty for a scheme root. Every other segment is non-empty and has
        /// no path traversal meaning.
        segments: Vec<String>,
    },
}

impl ResourcePath {
    pub fn parse(input: &str) -> Result<Self, ResourcePathError> {
        let Some((scheme, rest)) = input.split_once("://") else {
            return Ok(Self::Real(input.to_owned()));
        };
        if scheme.is_empty() {
            return Err(ResourcePathError::Malformed(input.to_owned()));
        }
        let scheme = ResourceScheme::try_from(scheme)?;
        if rest.contains('#') || rest.contains('?') || rest.contains('\\') {
            return Err(ResourcePathError::Malformed(input.to_owned()));
        }
        let segments = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split('/')
                .map(|segment| {
                    if segment.is_empty() || matches!(segment, "." | "..") {
                        Err(ResourcePathError::Malformed(input.to_owned()))
                    } else {
                        Ok(segment.to_owned())
                    }
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(Self::Virtual { scheme, segments })
    }

    pub fn is_scheme_root(&self) -> bool {
        matches!(self, Self::Virtual { segments, .. } if segments.is_empty())
    }
}

impl fmt::Display for ResourcePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Real(path) => f.write_str(path),
            Self::Virtual { scheme, segments } => {
                write!(f, "{scheme}://")?;
                f.write_str(&segments.join("/"))
            }
        }
    }
}

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
pub enum ResourcePathError {
    #[error(
        "unknown Artist virtual-path scheme `{0}`; use one of agent://, bash://, ask://, canvas://, computer://, process://, eval://, debug://, forge://, code://, relation://, rules://, artifact://, dict://, memory://, profile://, skill://, or tools://"
    )]
    UnknownScheme(String),
    #[error("malformed Artist resource path `{0}`")]
    Malformed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_paths_remain_opaque_to_the_virtual_parser() {
        assert_eq!(
            ResourcePath::parse("src/lib.rs").unwrap(),
            ResourcePath::Real("src/lib.rs".into())
        );
        assert_eq!(
            ResourcePath::parse("/tmp/project").unwrap(),
            ResourcePath::Real("/tmp/project".into())
        );
    }

    #[test]
    fn every_locked_scheme_has_one_canonical_root_form() {
        for scheme in ResourceScheme::ALL {
            let path = ResourcePath::parse(&format!("{scheme}://")).unwrap();
            assert!(path.is_scheme_root());
            assert_eq!(path.to_string(), format!("{scheme}://"));
        }
    }

    #[test]
    fn virtual_segments_cannot_escape_or_be_selector_syntax() {
        for value in [
            "agent:///goethe",
            "agent://goethe/../todo",
            "agent://goethe#x",
            "future://x",
        ] {
            assert!(ResourcePath::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn virtual_text_uses_stable_teca_line_anchors() {
        let rendered = render_anchored_virtual_text("one\ntwo\none");
        assert!(rendered.lines().all(|line| line.starts_with('#')));
        assert_eq!(rendered, render_anchored_virtual_text("one\ntwo\none"));
        assert_eq!(virtual_text_revision("one"), virtual_text_revision("one"));
    }
}
