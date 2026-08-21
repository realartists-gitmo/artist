//! Component-side provider catalogue.
//!
//! The catalogue is intentionally only a routing/factory layer. It does not
//! put provider-specific request policy into the daemon or kernel. Providers
//! whose concrete network adapter is not installed still expose a truthful
//! unsupported implementation rather than advertising capabilities they do
//! not have.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ResourceResolver;
use crate::{
    ModelCapabilities, ModelEvent, ModelEventStream, ModelProvider, ModelRequest, ProviderAuth,
    ProviderAuthenticator, ProviderComponent, ProviderConfig, ProviderConfigError, ProviderError,
    ProviderKind,
};

#[derive(Clone, Default)]
pub struct ProviderCatalog {
    components: BTreeMap<String, Arc<dyn ProviderComponent>>,
}

impl ProviderCatalog {
    pub fn builtins() -> Self {
        let mut catalog = Self::default();
        for kind in ProviderKind::ALL {
            catalog.register(Arc::new(BuiltinProviderComponent { kind }));
        }
        catalog
    }

    pub fn register(&mut self, component: Arc<dyn ProviderComponent>) {
        self.components
            .insert(component.provider_type().to_owned(), component);
    }

    pub fn provider_types(&self) -> Vec<String> {
        self.components.keys().cloned().collect()
    }

    pub fn configure(
        &self,
        config: &ProviderConfig,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        config.validate()?;
        self.components
            .get(&config.provider_type)
            .ok_or_else(|| ProviderConfigError::UnknownType(config.provider_type.clone()))?
            .configure(config)
    }

    /// Refreshes component-owned authentication before constructing a
    /// provider. A component may return the input unchanged when refresh is
    /// unnecessary; no credential is persisted by this method.
    pub async fn configure_refreshed(
        &self,
        config: &ProviderConfig,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        config.validate()?;
        let component = self
            .components
            .get(&config.provider_type)
            .ok_or_else(|| ProviderConfigError::UnknownType(config.provider_type.clone()))?;
        let mut config = config.clone();
        if let Some(authenticator) = component.authenticator(&config) {
            config.auth =
                authenticator
                    .refresh(&config.auth)
                    .await
                    .map_err(|error| match error {
                        ProviderError::Authentication { message } => {
                            ProviderConfigError::Authentication(message)
                        }
                        other => ProviderConfigError::Invalid(other.to_string()),
                    })?;
        }
        component.configure(&config)
    }

    pub async fn configure_refreshed_with_resource_resolver(
        &self,
        config: &ProviderConfig,
        resolver: Option<Arc<dyn ResourceResolver>>,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        config.validate()?;
        let component = self
            .components
            .get(&config.provider_type)
            .ok_or_else(|| ProviderConfigError::UnknownType(config.provider_type.clone()))?;
        let mut config = config.clone();
        if let Some(authenticator) = component.authenticator(&config) {
            config.auth =
                authenticator
                    .refresh(&config.auth)
                    .await
                    .map_err(|error| match error {
                        ProviderError::Authentication { message } => {
                            ProviderConfigError::Authentication(message)
                        }
                        other => ProviderConfigError::Invalid(other.to_string()),
                    })?;
        }
        component.configure_with_resource_resolver(&config, resolver)
    }
}

struct BuiltinProviderComponent {
    kind: ProviderKind,
}

impl ProviderComponent for BuiltinProviderComponent {
    fn provider_type(&self) -> &str {
        self.kind.as_str()
    }

    fn configure(
        &self,
        config: &ProviderConfig,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        self.configure_with_resource_resolver(config, None)
    }

    fn configure_with_resource_resolver(
        &self,
        config: &ProviderConfig,
        resolver: Option<Arc<dyn ResourceResolver>>,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        if config.provider_type != self.kind.as_str() {
            return Err(ProviderConfigError::UnknownType(
                config.provider_type.clone(),
            ));
        }

        let token = match &config.auth {
            ProviderAuth::ApiKey { secret } if !secret.trim().is_empty() => secret.clone(),
            ProviderAuth::ChatGptOAuth { access_token, .. } if !access_token.trim().is_empty() => {
                access_token.clone()
            }
            ProviderAuth::None => "local-component".into(),
            _ => {
                return Err(ProviderConfigError::Invalid(format!(
                    "{} requires a non-empty component credential",
                    self.kind.as_str()
                )));
            }
        };

        // Only the OpenAI component currently owns this concrete transport.
        // Other catalog entries are discoverable identities with an
        // explicitly unsupported provider implementation; they must not
        // advertise OpenAI capabilities merely because an endpoint exists.
        if self.kind != ProviderKind::OpenAi {
            return Ok(Arc::new(CatalogProvider {
                name: self.kind.as_str().to_owned(),
                inner: None,
            }));
        }
        let provider = crate::OpenAiProvider::new(token)
            .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?;
        let provider = if let Some(endpoint) = &config.endpoint {
            provider
                .with_endpoint(endpoint.clone())
                .map_err(|error| ProviderConfigError::Invalid(error.to_string()))?
        } else {
            provider
        };
        let provider = match resolver {
            Some(resolver) => provider.with_resource_resolver(resolver),
            None => provider,
        };
        Ok(Arc::new(CatalogProvider {
            name: self.kind.as_str().to_owned(),
            inner: Some(provider),
        }))
    }

    fn authenticator(&self, config: &ProviderConfig) -> Option<Arc<dyn ProviderAuthenticator>> {
        (self.kind == ProviderKind::ChatGpt).then(|| {
            Arc::new(ChatGptAuthenticator::from_config(config)) as Arc<dyn ProviderAuthenticator>
        })
    }
}

struct ChatGptAuthenticator {
    endpoint: String,
    client_id: String,
    client: reqwest::Client,
}

impl ChatGptAuthenticator {
    fn from_config(config: &ProviderConfig) -> Self {
        let endpoint = config
            .options
            .get("oauth_token_endpoint")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("https://auth.openai.com/oauth/token")
            .to_owned();
        let client_id = config
            .options
            .get("oauth_client_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("artist")
            .to_owned();
        Self {
            endpoint,
            client_id,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl ProviderAuthenticator for ChatGptAuthenticator {
    async fn refresh(&self, auth: &ProviderAuth) -> Result<ProviderAuth, ProviderError> {
        let ProviderAuth::ChatGptOAuth {
            access_token: _access_token,
            refresh_token,
            expires_at_ms,
        } = auth
        else {
            return Ok(auth.clone());
        };
        let Some(refresh_token) = refresh_token else {
            return Ok(auth.clone());
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if expires_at_ms.is_none_or(|expires| expires > now_ms.saturating_add(30_000)) {
            return Ok(auth.clone());
        }
        let response = self
            .client
            .post(&self.endpoint)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
            ])
            .send()
            .await
            .map_err(|error| ProviderError::Request {
                message: format!("OAuth refresh request failed: {error}"),
            })?;
        let status = response.status();
        let body: serde_json::Value =
            response
                .json()
                .await
                .map_err(|error| ProviderError::InvalidResponse {
                    message: format!("OAuth refresh response was not JSON: {error}"),
                })?;
        if !status.is_success() {
            return Err(ProviderError::Authentication {
                message: format!("OAuth refresh rejected with HTTP {}", status.as_u16()),
            });
        }
        let access_token = body
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| ProviderError::InvalidResponse {
                message: "OAuth refresh response omitted access_token".into(),
            })?;
        let expires_at_ms = body
            .get("expires_in")
            .and_then(serde_json::Value::as_u64)
            .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000)));
        Ok(ProviderAuth::ChatGptOAuth {
            access_token: access_token.to_owned(),
            refresh_token: Some(refresh_token.clone()),
            expires_at_ms,
        })
    }
}

struct CatalogProvider {
    name: String,
    inner: Option<crate::OpenAiProvider>,
}

impl ModelProvider for CatalogProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self, model: &str) -> ModelCapabilities {
        self.inner
            .as_ref()
            .map(|provider| provider.capabilities(model))
            .unwrap_or_default()
    }

    fn stream<'a>(&'a self, request: ModelRequest) -> ModelEventStream<'a> {
        if let Some(provider) = &self.inner {
            return provider.stream(request);
        }
        let message = format!("{} provider endpoint is not configured", self.name);
        Box::pin(futures_util::stream::once(async move {
            Err::<ModelEvent, ProviderError>(ProviderError::Unsupported { feature: message })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProviderConfig, ProviderConfigId};
    use serde_json::Value;

    #[test]
    fn builtins_have_stable_provider_types_without_false_capabilities() {
        let catalog = ProviderCatalog::builtins();
        assert_eq!(catalog.provider_types().len(), ProviderKind::ALL.len());
        let id = ProviderConfigId::new("local").unwrap();
        let provider = catalog
            .configure(&ProviderConfig {
                id,
                provider_type: "ollama".into(),
                endpoint: None,
                default_model: None,
                options: Value::Null,
                auth: ProviderAuth::None,
            })
            .unwrap();
        assert!(!provider.capabilities("model").streaming);
    }
}
