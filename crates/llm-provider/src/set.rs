//! The user's configured provider accounts.
//!
//! A profile names an account, not a provider kind: a bare model string cannot
//! disambiguate two ChatGPT logins, two Azure deployments, or Responses versus
//! Chat Completions on the same OpenAI key. This is the lookup that turns a
//! profile's `provider:` reference into a concrete [`SavedProvider`].

use crate::SavedProvider;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct ProviderSet {
    providers: Arc<Vec<SavedProvider>>,
}

impl ProviderSet {
    pub fn new(providers: Vec<SavedProvider>) -> Self {
        Self {
            providers: Arc::new(providers),
        }
    }

    /// Resolve a profile's `provider:` reference. Ids are matched first and are
    /// unique by construction; display names are matched case-insensitively and
    /// are not guaranteed unique, so an ambiguous name is an error rather than
    /// an arbitrary pick.
    pub fn resolve(&self, reference: &str) -> Result<&SavedProvider, String> {
        if let Some(found) = self
            .providers
            .iter()
            .find(|provider| provider.id.as_str() == reference)
        {
            return Ok(found);
        }
        let mut matches = self
            .providers
            .iter()
            .filter(|provider| provider.name.eq_ignore_ascii_case(reference));
        let first = matches.next().ok_or_else(|| {
            format!(
                "unknown provider: {reference}; configured providers: {}",
                self.references().join(", ")
            )
        })?;
        if matches.next().is_some() {
            return Err(format!(
                "provider name {reference} is ambiguous; use one of these ids instead: {}",
                self.ids_named(reference).join(", ")
            ));
        }
        Ok(first)
    }

    /// Ids and display names, for diagnostics.
    pub fn references(&self) -> Vec<String> {
        self.providers
            .iter()
            .map(|provider| format!("{} ({})", provider.id.as_str(), provider.name))
            .collect()
    }

    fn ids_named(&self, name: &str) -> Vec<String> {
        self.providers
            .iter()
            .filter(|provider| provider.name.eq_ignore_ascii_case(name))
            .map(|provider| provider.id.as_str().to_owned())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Auth, ProviderId, Secret};

    fn provider(id: &str, name: &str) -> SavedProvider {
        SavedProvider::chatgpt(
            ProviderId::new(id).unwrap(),
            name,
            Auth {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: "acct".into(),
                email: None,
                expires_at: None,
            },
        )
    }

    #[test]
    fn resolves_by_id_then_by_name() {
        let set = ProviderSet::new(vec![
            provider("work", "Anthropic"),
            provider("personal", "ChatGPT Personal"),
        ]);
        assert_eq!(set.resolve("work").unwrap().id.as_str(), "work");
        assert_eq!(
            set.resolve("chatgpt personal").unwrap().id.as_str(),
            "personal"
        );
    }

    /// An id always wins, even when another account's display name collides
    /// with it — otherwise renaming an account could silently reroute traffic.
    #[test]
    fn ids_take_precedence_over_names() {
        let set = ProviderSet::new(vec![provider("work", "personal"), provider("personal", "X")]);
        assert_eq!(set.resolve("personal").unwrap().id.as_str(), "personal");
    }

    #[test]
    fn ambiguous_names_are_an_error() {
        let set = ProviderSet::new(vec![provider("a", "Local"), provider("b", "local")]);
        let error = set.resolve("Local").unwrap_err();
        assert!(error.contains("ambiguous"), "{error}");
        assert!(error.contains('a') && error.contains('b'), "{error}");
    }

    #[test]
    fn unknown_references_list_what_is_configured() {
        let set = ProviderSet::new(vec![provider("work", "Anthropic")]);
        let error = set.resolve("nope").unwrap_err();
        assert!(error.contains("unknown provider"), "{error}");
        assert!(error.contains("work (Anthropic)"), "{error}");
    }

    #[test]
    fn an_empty_set_still_reports_cleanly() {
        let set = ProviderSet::default();
        assert!(set.is_empty());
        assert!(set.resolve("anything").is_err());
    }
}
