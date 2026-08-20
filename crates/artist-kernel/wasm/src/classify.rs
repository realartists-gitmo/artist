//! Classification of a wasm component by the contract families it satisfies.
//!
//! A component's class is determined purely by inspecting its exported WIT
//! interface names — no manifest. An extension exports one or more of:
//!
//! * `artist:nouns/provider` → noun (addressable, URI-aware resource)
//! * `artist:verbs/*` → verb (typed operations over resources; batch-native)
//! * `artist:events/subscriber` → event (long-lived, reactive service)
//!
//! A component may satisfy several families at once; its class is the union.

use wasmtime::Engine;
use wasmtime::component::Component;

/// The contract interface names that classify a component.
pub mod names {
    /// A noun exports the URI-aware provider interface.
    pub const NOUN: &str = "artist:nouns/provider";
    /// An event service exports the subscriber interface.
    pub const EVENT: &str = "artist:events/subscriber";
    /// All verb interfaces live under this prefix (specific verbs are defined
    /// in tool extensions' own packages).
    pub const VERB_PREFIX: &str = "artist:verbs/";
}

/// The set of contract families an extension satisfies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtensionClass {
    pub noun: bool,
    pub verb: bool,
    pub event: bool,
}

impl ExtensionClass {
    pub fn is_empty(&self) -> bool {
        !self.noun && !self.verb && !self.event
    }

    pub fn contains(&self, other: &ExtensionClass) -> bool {
        (other.noun && self.noun || !other.noun)
            && (other.verb && self.verb || !other.verb)
            && (other.event && self.event || !other.event)
    }
}

impl std::fmt::Display for ExtensionClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.noun {
            parts.push("noun");
        }
        if self.verb {
            parts.push("verb");
        }
        if self.event {
            parts.push("event");
        }
        if parts.is_empty() {
            write!(f, "unknown")
        } else {
            write!(f, "{}", parts.join("+"))
        }
    }
}

fn classify_export(name: &str) -> Option<ExtensionClass> {
    if name == names::NOUN
        || name
            .strip_prefix(names::NOUN)
            .is_some_and(|suffix| suffix.starts_with('@'))
    {
        Some(ExtensionClass {
            noun: true,
            ..Default::default()
        })
    } else if name == names::EVENT
        || name
            .strip_prefix(names::EVENT)
            .is_some_and(|suffix| suffix.starts_with('@'))
    {
        Some(ExtensionClass {
            event: true,
            ..Default::default()
        })
    } else if name.starts_with(names::VERB_PREFIX) {
        Some(ExtensionClass {
            verb: true,
            ..Default::default()
        })
    } else {
        None
    }
}

/// Classify a compiled component by its exported interfaces.
pub fn classify(engine: &Engine, component: &Component) -> ExtensionClass {
    let mut class = ExtensionClass::default();
    let ty = component.component_type();
    for (name, _) in ty.exports(engine) {
        if let Some(c) = classify_export(name) {
            class.noun |= c.noun;
            class.verb |= c.verb;
            class.event |= c.event;
        }
    }
    class
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_names_classify() {
        assert_eq!(
            classify_export("artist:nouns/provider@2.0.0"),
            Some(ExtensionClass {
                noun: true,
                ..Default::default()
            })
        );
        assert_eq!(
            classify_export("artist:events/subscriber@1.0.0"),
            Some(ExtensionClass {
                event: true,
                ..Default::default()
            })
        );
        assert_eq!(
            classify_export("artist:verbs/read@1.0.0"),
            Some(ExtensionClass {
                verb: true,
                ..Default::default()
            })
        );
        assert_eq!(classify_export("wasi:cli/run@0.3.0"), None);
        assert_eq!(classify_export("artist:nouns/other@1"), None);
    }

    #[test]
    fn class_is_union() {
        let a = ExtensionClass {
            noun: true,
            ..Default::default()
        };
        let b = ExtensionClass {
            event: true,
            ..Default::default()
        };
        let both = ExtensionClass {
            noun: true,
            event: true,
            ..Default::default()
        };
        assert!(both.contains(&a));
        assert!(both.contains(&b));
        assert!(!both.contains(&ExtensionClass {
            verb: true,
            ..Default::default()
        }));
    }
}
