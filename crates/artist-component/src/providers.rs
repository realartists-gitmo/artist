//! Provider configuration/auth component socket.
//!
//! Provider implementations are selected by the active `provider://...`
//! component routes. This socket owns the component factories and the
//! redacted configuration map; the daemon and kernel only carry stable
//! configuration identities.

use std::collections::BTreeMap;
use std::env;
use std::sync::{Arc, RwLock};

use llm_provider::{
    ModelProvider, OpenAiProvider, ProviderAuth, ProviderComponent, ProviderConfig,
    ProviderConfigError, ProviderError, ProviderKind, ResourceResolver,
};

#[derive(Clone)]
pub struct ProviderSocket {
    components: Arc<RwLock<BTreeMap<String, Arc<dyn ProviderComponent>>>>,
    available_components: Arc<RwLock<BTreeMap<String, Arc<dyn ProviderComponent>>>>,
    configuration: Arc<dyn ProviderConfigurationComponent>,
    active_routes: Arc<RwLock<Option<std::collections::BTreeSet<String>>>>,
}

impl ProviderSocket {
    pub fn new(components: impl IntoIterator<Item = Arc<dyn ProviderComponent>>) -> Self {
        Self::with_configuration(
            components,
            Arc::new(DefaultProviderConfigurationComponent::default()),
        )
    }

    pub fn with_configuration(
        components: impl IntoIterator<Item = Arc<dyn ProviderComponent>>,
        configuration: Arc<dyn ProviderConfigurationComponent>,
    ) -> Self {
        let mut values = BTreeMap::new();
        for component in components {
            values.insert(component.provider_type().to_owned(), component);
        }
        Self {
            components: Arc::new(RwLock::new(values.clone())),
            available_components: Arc::new(RwLock::new(values)),
            configuration,
            active_routes: Arc::new(RwLock::new(None)),
        }
    }

    pub fn builtins() -> Self {
        Self::new([Arc::new(OpenAiProviderComponent) as Arc<dyn ProviderComponent>])
    }

    /// Restrict this socket to the provider implementations claimed by the
    /// active URL/component graph. Configuration state remains shared so a
    /// later composition reload does not copy or expose secrets.
    pub fn for_routes(&self, routes: &std::collections::BTreeSet<String>) -> Self {
        let available_components = Arc::clone(&self.available_components);
        let components = available_components
            .read()
            .unwrap()
            .iter()
            .filter(|(kind, _)| routes.contains(&format!("provider://{kind}")))
            .map(|(kind, component)| (kind.clone(), Arc::clone(component)))
            .collect();
        Self {
            components: Arc::new(RwLock::new(components)),
            available_components,
            configuration: Arc::clone(&self.configuration),
            active_routes: Arc::new(RwLock::new(Some(routes.clone()))),
        }
    }

    /// Reconcile the active implementation set after a URL composition
    /// generation changes. The available component factories are retained;
    /// only implementations claimed by the new graph become selectable.
    pub fn refresh_for_routes(&self, routes: &std::collections::BTreeSet<String>) {
        let available = self.available_components.read().unwrap();
        let selected = available
            .iter()
            .filter(|(kind, _)| routes.contains(&format!("provider://{kind}")))
            .map(|(kind, component)| (kind.clone(), Arc::clone(component)))
            .collect();
        *self.components.write().unwrap() = selected;
        *self.active_routes.write().unwrap() = Some(routes.clone());
    }

    pub fn register_component(&self, component: Arc<dyn ProviderComponent>) {
        let kind = component.provider_type().to_owned();
        self.available_components
            .write()
            .unwrap()
            .insert(kind.clone(), Arc::clone(&component));
        let active = self.active_routes.read().unwrap();
        if active
            .as_ref()
            .is_none_or(|routes| routes.contains(&format!("provider://{kind}")))
        {
            self.components.write().unwrap().insert(kind, component);
        }
    }

    pub fn provider_types(&self) -> Vec<String> {
        self.components.read().unwrap().keys().cloned().collect()
    }

    pub fn from_environment() -> Self {
        Self::with_configuration(
            [Arc::new(OpenAiProviderComponent) as Arc<dyn ProviderComponent>],
            Arc::new(DefaultProviderConfigurationComponent::from_environment()),
        )
    }

    pub fn register(&self, config: ProviderConfig) -> Result<(), ProviderConfigError> {
        self.configuration.register(config)
    }

    pub fn configuration(&self, id: &str) -> Result<ProviderConfig, ProviderConfigError> {
        self.configuration.configuration(id)
    }

    pub fn select(
        &self,
        requested: Option<&str>,
        provider_type: Option<&str>,
    ) -> Result<ProviderConfig, ProviderConfigError> {
        self.configuration.select(requested, provider_type)
    }

    pub fn default_model(&self, id: &str) -> Option<String> {
        self.configuration.default_model(id)
    }

    pub async fn configure(
        &self,
        config: &ProviderConfig,
        resolver: Option<Arc<dyn ResourceResolver>>,
    ) -> Result<Arc<dyn ModelProvider>, ProviderConfigError> {
        config.validate()?;
        let component = self
            .components
            .read()
            .unwrap()
            .get(&config.provider_type)
            .cloned()
            .ok_or_else(|| ProviderConfigError::UnknownType(config.provider_type.clone()))?;
        let mut refreshed = config.clone();
        if let Some(authenticator) = component.authenticator(&refreshed) {
            refreshed.auth = authenticator
                .refresh(&refreshed.auth)
                .await
                .map_err(provider_auth_error)?;
        }
        component.configure_with_resource_resolver(&refreshed, resolver)
    }

    pub fn configuration_component(&self) -> Arc<dyn ProviderConfigurationComponent> {
        Arc::clone(&self.configuration)
    }
}

fn unknown_configuration(id: &str) -> ProviderConfigError {
    ProviderConfigError::Unknown(
        llm_provider::ProviderConfigId::new(id.to_owned())
            .unwrap_or_else(|_| llm_provider::ProviderConfigId::new("invalid").unwrap()),
    )
}

fn provider_auth_error(error: ProviderError) -> ProviderConfigError {
    match error {
        ProviderError::Authentication { message } => ProviderConfigError::Authentication(message),
        other => ProviderConfigError::Invalid(other.to_string()),
    }
}

/// The shipped OpenAI transport is an Artist provider component. It is
/// installed in the socket as `provider://openai`; it is not a kernel or
/// daemon special case.
struct OpenAiProviderComponent;

impl ProviderComponent for OpenAiProviderComponent {
    fn provider_type(&self) -> &str {
        ProviderKind::OpenAi.as_str()
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
        if config.provider_type != self.provider_type() {
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
                return Err(ProviderConfigError::Invalid(
                    "openai requires a non-empty component credential".into(),
                ));
            }
        };
        let provider = OpenAiProvider::new(token)
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
        Ok(Arc::new(provider))
    }
}

/// Component-owned provider configuration/auth state. The runtime socket only
/// routes to this component; it does not own a second host-side configuration
/// registry or interpret environment credentials itself.
pub trait ProviderConfigurationComponent: Send + Sync {
    fn register(&self, config: ProviderConfig) -> Result<(), ProviderConfigError>;

    fn configuration(&self, id: &str) -> Result<ProviderConfig, ProviderConfigError>;

    fn select(
        &self,
        requested: Option<&str>,
        provider_type: Option<&str>,
    ) -> Result<ProviderConfig, ProviderConfigError>;

    fn default_model(&self, id: &str) -> Option<String>;
}

#[derive(Default)]
pub struct DefaultProviderConfigurationComponent {
    configurations: RwLock<BTreeMap<String, ProviderConfig>>,
}

impl DefaultProviderConfigurationComponent {
    pub fn from_environment() -> Self {
        let component = Self::default();
        let Ok(secret) = env::var("OPENAI_API_KEY") else {
            return component;
        };
        let id = llm_provider::ProviderConfigId::new("openai-default")
            .expect("static provider id is valid");
        let config = ProviderConfig {
            id,
            provider_type: ProviderKind::OpenAi.as_str().into(),
            endpoint: env::var("ARTIST_OPENAI_ENDPOINT").ok(),
            default_model: env::var("ARTIST_MODEL").ok(),
            options: serde_json::Value::Null,
            auth: ProviderAuth::ApiKey { secret },
        };
        let _ = component.register(config);
        component
    }
}

impl ProviderConfigurationComponent for DefaultProviderConfigurationComponent {
    fn register(&self, config: ProviderConfig) -> Result<(), ProviderConfigError> {
        config.validate()?;
        let id = config.id.to_string();
        let mut configurations = self.configurations.write().unwrap();
        if configurations.contains_key(&id) {
            return Err(ProviderConfigError::Duplicate(config.id));
        }
        configurations.insert(id, config);
        Ok(())
    }

    fn configuration(&self, id: &str) -> Result<ProviderConfig, ProviderConfigError> {
        self.configurations
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| unknown_configuration(id))
    }

    fn select(
        &self,
        requested: Option<&str>,
        provider_type: Option<&str>,
    ) -> Result<ProviderConfig, ProviderConfigError> {
        let configurations = self.configurations.read().unwrap();
        let id = if let Some(requested) = requested {
            requested.to_owned()
        } else {
            let candidates: Vec<_> = configurations
                .iter()
                .filter(|(_, config)| provider_type.is_none_or(|kind| config.provider_type == kind))
                .map(|(id, _)| id.clone())
                .collect();
            match candidates.as_slice() {
                [id] => id.clone(),
                [] => {
                    return Err(ProviderConfigError::Invalid(
                        provider_type
                            .map(|kind| format!("no provider configuration for {kind:?}"))
                            .unwrap_or_else(|| "no provider configuration is selected".into()),
                    ));
                }
                _ => {
                    return Err(ProviderConfigError::Invalid(
                        "multiple provider configurations match; select a stable provider id"
                            .into(),
                    ));
                }
            }
        };
        configurations
            .get(&id)
            .cloned()
            .ok_or_else(|| unknown_configuration(&id))
    }

    fn default_model(&self, id: &str) -> Option<String> {
        self.configurations
            .read()
            .unwrap()
            .get(id)
            .and_then(|config| config.default_model.clone())
    }
}
