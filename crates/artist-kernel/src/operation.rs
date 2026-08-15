use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Canonical identity of a versioned typed verb contract.
///
/// The kernel is being migrated from the closed `Verb` enum to this identity.
/// The string is deliberately retained as the contract identity rather than
/// reduced to a display name.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct VerbId(String);

impl VerbId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let (contract, version) = value
            .rsplit_once('@')
            .ok_or_else(|| "verb identity must contain @version".to_owned())?;
        let parts = contract.split('/').collect::<Vec<_>>();
        if parts.len() != 2
            || parts[0].split(':').count() != 2
            || parts.iter().any(|part| part.is_empty())
            || version.split('.').count() != 3
            || version.split('.').any(|part| part.parse::<u64>().is_err())
        {
            return Err(format!("invalid canonical verb identity: {value}"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn contract(&self) -> &str {
        self.0.rsplit_once('/').unwrap().0
    }

    pub fn package(&self) -> &str {
        self.contract().split_once(':').unwrap().0
    }

    pub fn interface(&self) -> &str {
        self.contract().split_once(':').unwrap().1
    }

    pub fn function(&self) -> &str {
        self.0
            .rsplit_once('/')
            .and_then(|(_, function_and_version)| function_and_version.rsplit_once('@'))
            .map(|(function, _)| function)
            .unwrap()
    }

    pub fn contract_versioned(&self) -> String {
        format!("{}@{}", self.contract(), self.version())
    }

    pub fn version(&self) -> &str {
        self.0.rsplit_once('@').unwrap().1
    }
}

impl fmt::Display for VerbId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for VerbId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::VerbId;
    use std::str::FromStr;

    #[test]
    fn canonical_versioned_contract_identity_is_preserved() {
        let id = VerbId::from_str("thirdparty:render/render@1.0.0").unwrap();
        assert_eq!(id.as_str(), "thirdparty:render/render@1.0.0");
        assert_eq!(id.to_string(), "thirdparty:render/render@1.0.0");
        assert_eq!(id.contract(), "thirdparty:render");
        assert_eq!(id.package(), "thirdparty");
        assert_eq!(id.interface(), "render");
        assert_eq!(id.function(), "render");
        assert_eq!(id.version(), "1.0.0");
        assert_eq!(id.contract_versioned(), "thirdparty:render@1.0.0");
    }

    #[test]
    fn rejects_unversioned_or_ambiguous_verb_identity() {
        assert!(VerbId::new("read").is_err());
        assert!(VerbId::new("artist:read/read@1").is_err());
        assert!(VerbId::new("artist:read/read@1.0.x").is_err());
    }
}
