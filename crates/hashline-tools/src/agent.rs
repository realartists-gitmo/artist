use uuid::Uuid;

/// Opaque agent identifier. Any non-empty string is accepted so host harnesses
/// can reuse their own IDs; `AgentId::new()` mints a UUID v4.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Accept any non-empty agent id from the host harness.
    pub fn from_string(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err("agent ID must be non-empty".into());
        }
        Ok(Self(value))
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of the agent performing a file operation. Anchor state is scoped
/// per agent so concurrent agents do not clobber each other's issued tokens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentIdentity {
    pub id: AgentId,
}

impl AgentIdentity {
    pub fn new() -> Self {
        Self { id: AgentId::new() }
    }

    pub fn from_id(id: impl Into<String>) -> Result<Self, String> {
        Ok(Self {
            id: AgentId::from_string(id)?,
        })
    }
}

impl Default for AgentIdentity {
    fn default() -> Self {
        Self::new()
    }
}
