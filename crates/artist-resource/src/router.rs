use std::sync::{Arc, RwLock};

use globset::{Glob, GlobMatcher};
use tokio::sync::watch;

use crate::{
    ResourceError, ResourceMetadata, ResourceOperation, ResourceProvider, ResourceReply,
    ResourceRequest, ResourceRoute, ResourceUri,
};

#[derive(Clone)]
pub struct ResourceRouter {
    inner: Arc<RouterInner>,
}

struct RouterInner {
    routes: RwLock<Vec<RegisteredRoute>>,
    topology: watch::Sender<u64>,
}

impl Default for ResourceRouter {
    fn default() -> Self {
        let (topology, _) = watch::channel(0);
        Self {
            inner: Arc::new(RouterInner {
                routes: RwLock::new(Vec::new()),
                topology,
            }),
        }
    }
}

struct RegisteredRoute {
    plugin: String,
    order: usize,
    declaration: ResourceRoute,
    base: GlobMatcher,
    projection: Option<GlobMatcher>,
    literals: usize,
    wildcards: usize,
    provider: Arc<dyn ResourceProvider>,
}

impl ResourceRouter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn generation(&self) -> u64 {
        *self.inner.topology.borrow()
    }

    /// Observe route registrations and successful topology-changing requests.
    pub fn subscribe_generation(&self) -> watch::Receiver<u64> {
        self.inner.topology.subscribe()
    }

    fn changed(&self) {
        self.inner
            .topology
            .send_modify(|generation| *generation += 1);
    }

    pub async fn register(
        &self,
        plugin: impl Into<String>,
        route: ResourceRoute,
        provider: Arc<dyn ResourceProvider>,
    ) -> Result<(), ResourceError> {
        validate_declaration(&route)?;
        let base = Glob::new(&route.base_glob)
            .map_err(|e| ResourceError::Invalid(format!("invalid base glob: {e}")))?
            .compile_matcher();
        let projection = route
            .projection_glob
            .as_deref()
            .map(Glob::new)
            .transpose()
            .map_err(|e| ResourceError::Invalid(format!("invalid projection glob: {e}")))?
            .map(|g| g.compile_matcher());
        let (literals, wildcards) = specificity(&route.base_glob, route.projection_glob.as_deref());
        let mut routes = self
            .inner
            .routes
            .write()
            .expect("resource route lock poisoned");
        let order = routes.len();
        routes.push(RegisteredRoute {
            plugin: plugin.into(),
            order,
            declaration: route,
            base,
            projection,
            literals,
            wildcards,
            provider,
        });
        drop(routes);
        self.changed();
        Ok(())
    }

    /// Replace every route owned by one plugin after compiling the complete
    /// candidate set. A malformed candidate cannot remove active routes.
    pub fn replace_owner(
        &self,
        owner: &str,
        replacements: Vec<(ResourceRoute, Arc<dyn ResourceProvider>)>,
    ) -> Result<(), ResourceError> {
        let mut compiled = Vec::with_capacity(replacements.len());
        for (declaration, provider) in replacements {
            validate_declaration(&declaration)?;
            let base = Glob::new(&declaration.base_glob)
                .map_err(|error| ResourceError::Invalid(format!("invalid base glob: {error}")))?
                .compile_matcher();
            let projection = declaration
                .projection_glob
                .as_deref()
                .map(Glob::new)
                .transpose()
                .map_err(|error| {
                    ResourceError::Invalid(format!("invalid projection glob: {error}"))
                })?
                .map(|glob| glob.compile_matcher());
            let (literals, wildcards) = specificity(
                &declaration.base_glob,
                declaration.projection_glob.as_deref(),
            );
            compiled.push((declaration, base, projection, literals, wildcards, provider));
        }
        let mut routes = self
            .inner
            .routes
            .write()
            .expect("resource route lock poisoned");
        routes.retain(|route| route.plugin != owner);
        let first_order = routes
            .iter()
            .map(|route| route.order)
            .max()
            .map_or(0, |n| n + 1);
        for (offset, (declaration, base, projection, literals, wildcards, provider)) in
            compiled.into_iter().enumerate()
        {
            routes.push(RegisteredRoute {
                plugin: owner.to_owned(),
                order: first_order + offset,
                declaration,
                base,
                projection,
                literals,
                wildcards,
                provider,
            });
        }
        drop(routes);
        self.changed();
        Ok(())
    }

    pub fn route_owner(&self, uri: &ResourceUri, operation: ResourceOperation) -> Option<String> {
        self.select(uri, operation)
            .map(|route| route.plugin.clone())
    }

    pub async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        validate_request(&request)?;
        if let ResourceRequest::Read {
            uri,
            start_line,
            line_count,
        } = &request
            && let Some(target) = metadata_target(uri)?
        {
            let metadata = self.metadata(&target)?;
            let text = serde_json::to_string_pretty(&metadata)
                .map_err(|error| ResourceError::Provider(error.to_string()))?;
            return Ok(ResourceReply::Text {
                text: slice_lines(&text, *start_line, *line_count),
            });
        }
        let operation = request.operation();
        let uri = request.uri().clone();
        let route = match self.select(&uri, operation) {
            Some(route) => route,
            None if self.has_route(&uri) => {
                return Err(ResourceError::Unsupported { uri, operation });
            }
            None => return Err(ResourceError::NotFound { uri, operation }),
        };
        if let ResourceRequest::Move { to: Some(to), .. } = &request {
            let destination = self.select(to, operation);
            if destination
                .as_ref()
                .is_none_or(|other| !Arc::ptr_eq(&route.provider, &other.provider))
            {
                return Err(ResourceError::Unsupported {
                    uri: uri.clone(),
                    operation,
                });
            }
        }
        if let ResourceRequest::Signal { name, payload, .. } = &request {
            let signal = route
                .signals
                .iter()
                .find(|signal| signal.name == *name)
                .ok_or_else(|| {
                    ResourceError::Invalid(format!(
                        "signal `{name}` is not declared by the selected route"
                    ))
                })?;
            let payload = payload
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|error| {
                    ResourceError::Invalid(format!("signal payload is not valid JSON: {error}"))
                })?
                .unwrap_or(serde_json::Value::Null);
            let validator = jsonschema::validator_for(&signal.payload_schema).map_err(|error| {
                ResourceError::Invalid(format!("invalid signal payload schema: {error}"))
            })?;
            if let Err(error) = validator.validate(&payload) {
                return Err(ResourceError::Invalid(format!(
                    "payload for signal `{name}` does not match its schema: {error}"
                )));
            }
        }
        let result = route.provider.handle(request).await;
        if result.is_ok()
            && matches!(
                operation,
                ResourceOperation::Write
                    | ResourceOperation::Edit
                    | ResourceOperation::Move
                    | ResourceOperation::Run
                    | ResourceOperation::Signal
            )
        {
            self.changed();
        }
        result
    }

    /// Synthesize the public capabilities of the selected routes for `uri`.
    pub fn metadata(&self, uri: &ResourceUri) -> Result<ResourceMetadata, ResourceError> {
        let operations = ResourceOperation::ALL
            .into_iter()
            .filter(|operation| self.select(uri, *operation).is_some())
            .collect::<Vec<_>>();
        if operations.is_empty() && !self.has_route(uri) {
            return Err(ResourceError::NotFound {
                uri: uri.clone(),
                operation: ResourceOperation::Read,
            });
        }
        let mut signals = self
            .select(uri, ResourceOperation::Signal)
            .map(|selection| selection.signals)
            .unwrap_or_default();
        signals.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ResourceMetadata {
            uri: uri.clone(),
            operations,
            signals,
        })
    }

    fn select(&self, uri: &ResourceUri, operation: ResourceOperation) -> Option<RouteSelection> {
        let routes = self
            .inner
            .routes
            .read()
            .expect("resource route lock poisoned");
        routes
            .iter()
            .filter(|route| {
                route.declaration.operations.contains(&operation) && route.matches(uri, operation)
            })
            .max_by_key(|route| {
                (
                    route.literals,
                    std::cmp::Reverse(route.wildcards),
                    std::cmp::Reverse(route.order),
                )
            })
            .map(|route| RouteSelection {
                plugin: route.plugin.clone(),
                provider: route.provider.clone(),
                signals: route.declaration.signals.clone(),
            })
    }

    fn has_route(&self, uri: &ResourceUri) -> bool {
        self.inner
            .routes
            .read()
            .expect("resource route lock poisoned")
            .iter()
            .any(|route| route.matches_node(uri))
    }

    pub fn routes(&self) -> Vec<(String, ResourceRoute)> {
        self.inner
            .routes
            .read()
            .expect("resource route lock poisoned")
            .iter()
            .map(|r| (r.plugin.clone(), r.declaration.clone()))
            .collect()
    }

    pub fn schemes(&self) -> Vec<String> {
        let mut schemes = self
            .inner
            .routes
            .read()
            .expect("resource route lock poisoned")
            .iter()
            .filter_map(|route| route.declaration.base_glob.split_once(':').map(|(s, _)| s))
            .filter(|scheme| {
                !scheme
                    .bytes()
                    .any(|b| matches!(b, b'*' | b'?' | b'[' | b']'))
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        schemes.sort();
        schemes.dedup();
        schemes
    }

    /// Literal first projection segments advertised for a base URI.
    pub fn projection_roots(&self, base: &ResourceUri) -> Vec<String> {
        let routes = self
            .inner
            .routes
            .read()
            .expect("resource route lock poisoned");
        let mut roots = routes
            .iter()
            .filter(|route| route.base.is_match(base.base().to_string()))
            .filter_map(|route| {
                let first = route
                    .declaration
                    .projection_glob
                    .as_deref()?
                    .split('/')
                    .next()?;
                (!first.is_empty()
                    && !first
                        .bytes()
                        .any(|b| matches!(b, b'*' | b'?' | b'[' | b']' | b'{')))
                .then(|| first.to_owned())
            })
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        roots
    }
}

fn validate_declaration(route: &ResourceRoute) -> Result<(), ResourceError> {
    if route
        .projection_glob
        .as_deref()
        .and_then(|projection| projection.split('/').next())
        == Some("meta")
    {
        return Err(ResourceError::Invalid(
            "the `meta` projection root is reserved by the kernel".into(),
        ));
    }
    if !route.signals.is_empty() && !route.operations.contains(&ResourceOperation::Signal) {
        return Err(ResourceError::Invalid(
            "signal definitions require the signal operation".into(),
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    for signal in &route.signals {
        if signal.name.is_empty() {
            return Err(ResourceError::Invalid(
                "signal names must not be empty".into(),
            ));
        }
        if !names.insert(&signal.name) {
            return Err(ResourceError::Invalid(format!(
                "signal `{}` is declared more than once",
                signal.name
            )));
        }
        jsonschema::validator_for(&signal.payload_schema).map_err(|error| {
            ResourceError::Invalid(format!(
                "invalid payload schema for signal `{}`: {error}",
                signal.name
            ))
        })?;
    }
    Ok(())
}

fn validate_request(request: &ResourceRequest) -> Result<(), ResourceError> {
    match request {
        ResourceRequest::Edit {
            expected_sha256,
            replacements,
            ..
        } => {
            if replacements.is_empty() {
                return Err(ResourceError::Invalid(
                    "an edit must contain at least one replacement".into(),
                ));
            }
            if expected_sha256.len() != 64
                || !expected_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(ResourceError::Invalid(
                    "expected_sha256 must be 64 lowercase hexadecimal characters".into(),
                ));
            }
        }
        ResourceRequest::Run {
            timeout: Some(timeout),
            ..
        } if timeout.is_zero() => {
            return Err(ResourceError::Invalid(
                "run timeout must be positive".into(),
            ));
        }
        ResourceRequest::Signal { name, .. } if name.is_empty() => {
            return Err(ResourceError::Invalid(
                "signal name must not be empty".into(),
            ));
        }
        _ => {}
    }
    Ok(())
}

struct RouteSelection {
    plugin: String,
    provider: Arc<dyn ResourceProvider>,
    signals: Vec<crate::SignalDefinition>,
}

fn metadata_target(uri: &ResourceUri) -> Result<Option<ResourceUri>, ResourceError> {
    let segments = uri.projection_segments();
    if segments.first().map(String::as_str) != Some("meta") {
        return Ok(None);
    }
    let mut target = uri.base();
    for segment in &segments[1..] {
        target = target
            .descend_projection(segment)
            .map_err(|error| ResourceError::Invalid(error.to_string()))?;
    }
    Ok(Some(target))
}

fn slice_lines(text: &str, start: Option<u64>, count: Option<u64>) -> String {
    if start.is_none() && count.is_none() {
        return text.to_owned();
    }
    let start = start.unwrap_or(1).saturating_sub(1) as usize;
    text.split_inclusive('\n')
        .skip(start)
        .take(count.unwrap_or(u64::MAX) as usize)
        .collect()
}
impl RegisteredRoute {
    fn matches_node(&self, uri: &ResourceUri) -> bool {
        matches_uri(&self.base, self.projection.as_ref(), uri) || self.matches_projection_root(uri)
    }

    fn matches(&self, uri: &ResourceUri, operation: ResourceOperation) -> bool {
        matches_uri(&self.base, self.projection.as_ref(), uri)
            || operation == ResourceOperation::Children && self.matches_projection_root(uri)
    }

    fn matches_projection_root(&self, uri: &ResourceUri) -> bool {
        self.base.is_match(uri.base().to_string())
            && self
                .declaration
                .projection_glob
                .as_deref()
                .and_then(|glob| glob.split('/').next())
                .is_some_and(|root| uri.projection_segments() == [root])
    }
}

fn matches_uri(base: &GlobMatcher, projection: Option<&GlobMatcher>, uri: &ResourceUri) -> bool {
    if !base.is_match(uri.base().to_string()) {
        return false;
    }
    let path = uri.projection_segments().join("/");
    match projection {
        Some(glob) => !path.is_empty() && glob.is_match(path),
        None => path.is_empty(),
    }
}

fn specificity(base: &str, projection: Option<&str>) -> (usize, usize) {
    let segments = base
        .split('/')
        .chain(projection.into_iter().flat_map(|s| s.split('/')));
    segments.fold((0, 0), |(l, w), s| {
        if s.bytes()
            .any(|b| matches!(b, b'*' | b'?' | b'[' | b']' | b'{'))
        {
            (l, w + 1)
        } else {
            (l + 1, w)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvironmentEntry, ResourceOperation::Read, ResourceReply, SignalDefinition};
    use async_trait::async_trait;
    use serde_json::json;

    struct Text(&'static str);
    #[async_trait]
    impl ResourceProvider for Text {
        async fn handle(&self, _: ResourceRequest) -> Result<ResourceReply, ResourceError> {
            Ok(ResourceReply::Text {
                text: self.0.into(),
            })
        }
    }
    fn uri(s: &str) -> ResourceUri {
        ResourceUri::resolve(s, std::path::Path::new("/work")).unwrap()
    }

    #[tokio::test]
    async fn chooses_specificity_then_load_order() {
        let router = ResourceRouter::new();
        router
            .register(
                "broad",
                ResourceRoute::new("file:///**", None::<String>, [Read]),
                Arc::new(Text("broad")),
            )
            .await
            .unwrap();
        router
            .register(
                "specific",
                ResourceRoute::new("file:///work/src/*.rs", None::<String>, [Read]),
                Arc::new(Text("specific")),
            )
            .await
            .unwrap();
        let reply = router
            .handle(ResourceRequest::Read {
                uri: uri("src/lib.rs"),
                start_line: None,
                line_count: None,
            })
            .await
            .unwrap();
        assert_eq!(
            reply,
            ResourceReply::Text {
                text: "specific".into()
            }
        );

        let tied = ResourceRouter::new();
        tied.register(
            "first",
            ResourceRoute::new("file:///**", None::<String>, [Read]),
            Arc::new(Text("first")),
        )
        .await
        .unwrap();
        tied.register(
            "second",
            ResourceRoute::new("file:///**", None::<String>, [Read]),
            Arc::new(Text("second")),
        )
        .await
        .unwrap();
        assert_eq!(tied.route_owner(&uri("x"), Read).as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn distinguishes_projection_routes() {
        let router = ResourceRouter::new();
        router
            .register(
                "file",
                ResourceRoute::new("file:///**", None::<String>, [Read]),
                Arc::new(Text("file")),
            )
            .await
            .unwrap();
        router
            .register(
                "ast",
                ResourceRoute::new("file:///**/*.rs", Some("symbols/**"), [Read]),
                Arc::new(Text("ast")),
            )
            .await
            .unwrap();
        assert_eq!(
            router
                .route_owner(&uri("src/lib.rs?symbols/foo"), Read)
                .as_deref(),
            Some("ast")
        );
        assert_eq!(
            router.route_owner(&uri("src/lib.rs"), Read).as_deref(),
            Some("file")
        );
        assert_eq!(
            router
                .handle(ResourceRequest::Read {
                    uri: uri("src/lib.rs"),
                    start_line: None,
                    line_count: None,
                })
                .await
                .unwrap(),
            ResourceReply::Text {
                text: "file".into()
            }
        );
        assert_eq!(
            router
                .handle(ResourceRequest::Read {
                    uri: uri("src/lib.rs?symbols/foo"),
                    start_line: None,
                    line_count: None,
                })
                .await
                .unwrap(),
            ResourceReply::Text { text: "ast".into() }
        );
    }

    #[tokio::test]
    async fn reports_unsupported_operation_on_existing_node() {
        let router = ResourceRouter::new();
        router
            .register(
                "read",
                ResourceRoute::new("file:///**", None::<String>, [Read]),
                Arc::new(Text("x")),
            )
            .await
            .unwrap();
        assert!(matches!(
            router
                .handle(ResourceRequest::Write {
                    uri: uri("a"),
                    text: "x".into()
                })
                .await,
            Err(ResourceError::Unsupported {
                operation: ResourceOperation::Write,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn move_can_span_two_routes_of_one_provider() {
        let router = ResourceRouter::new();
        let provider: Arc<dyn ResourceProvider> = Arc::new(Text("moved"));
        router
            .register(
                "same",
                ResourceRoute::new(
                    "file:///work/a/**",
                    None::<String>,
                    [ResourceOperation::Move],
                ),
                provider.clone(),
            )
            .await
            .unwrap();
        router
            .register(
                "same",
                ResourceRoute::new(
                    "file:///work/b/**",
                    None::<String>,
                    [ResourceOperation::Move],
                ),
                provider,
            )
            .await
            .unwrap();
        assert!(
            router
                .handle(ResourceRequest::Move {
                    from: uri("a/x"),
                    to: Some(uri("b/x"))
                })
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn move_cannot_span_distinct_providers() {
        let router = ResourceRouter::new();
        for (path, provider) in [
            (
                "file:///work/a/**",
                Arc::new(Text("a")) as Arc<dyn ResourceProvider>,
            ),
            (
                "file:///work/b/**",
                Arc::new(Text("b")) as Arc<dyn ResourceProvider>,
            ),
        ] {
            router
                .register(
                    path,
                    ResourceRoute::new(path, None::<String>, [ResourceOperation::Move]),
                    provider,
                )
                .await
                .unwrap();
        }
        assert!(matches!(
            router
                .handle(ResourceRequest::Move {
                    from: uri("a/x"),
                    to: Some(uri("b/x")),
                })
                .await,
            Err(ResourceError::Unsupported {
                operation: ResourceOperation::Move,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn publishes_conservative_topology_invalidations() {
        let router = ResourceRouter::new();
        let mut changes = router.subscribe_generation();
        router
            .register(
                "writer",
                ResourceRoute::new("file:///**", None::<String>, [ResourceOperation::Write]),
                Arc::new(Text("written")),
            )
            .await
            .unwrap();
        changes.changed().await.unwrap();
        assert_eq!(*changes.borrow_and_update(), 1);
        router
            .handle(ResourceRequest::Write {
                uri: uri("changed"),
                text: "body".into(),
            })
            .await
            .unwrap();
        changes.changed().await.unwrap();
        assert_eq!(*changes.borrow_and_update(), 2);

        router
            .register(
                "reader",
                ResourceRoute::new("mem:///**", None::<String>, [ResourceOperation::Read]),
                Arc::new(Text("body")),
            )
            .await
            .unwrap();
        changes.changed().await.unwrap();
        assert_eq!(*changes.borrow_and_update(), 3);

        assert!(matches!(
            router
                .handle(ResourceRequest::Read {
                    uri: uri("changed"),
                    start_line: None,
                    line_count: None,
                })
                .await,
            Err(ResourceError::Unsupported {
                operation: ResourceOperation::Read,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn successful_mutations_notify_topology_watchers_but_reads_do_not() {
        let cases = [
            (
                ResourceOperation::Edit,
                ResourceRequest::Edit {
                    uri: uri("changed"),
                    expected_sha256: "0".repeat(64),
                    replacements: vec![crate::TextReplacement {
                        start_byte: 0,
                        end_byte: 0,
                        text: "x".into(),
                    }],
                },
            ),
            (
                ResourceOperation::Move,
                ResourceRequest::Move {
                    from: uri("changed"),
                    to: None,
                },
            ),
            (
                ResourceOperation::Run,
                ResourceRequest::Run {
                    target: uri("changed"),
                    input: String::new(),
                    cwd: None,
                    env: Vec::new(),
                    timeout: None,
                },
            ),
        ];
        for (operation, request) in cases {
            let router = ResourceRouter::new();
            router
                .register(
                    "provider",
                    ResourceRoute::new("file:///**", None::<String>, [operation]),
                    Arc::new(Text("ok")),
                )
                .await
                .unwrap();
            let mut changes = router.subscribe_generation();
            assert!(router.handle(request).await.is_ok());
            changes.changed().await.unwrap();
        }

        let router = ResourceRouter::new();
        router
            .register(
                "signaler",
                ResourceRoute::new("file:///**", None::<String>, [ResourceOperation::Signal])
                    .with_signals(vec![SignalDefinition {
                        name: "refresh".into(),
                        description: "refresh topology".into(),
                        payload_schema: json!({"type": "null"}),
                    }]),
                Arc::new(Text("ok")),
            )
            .await
            .unwrap();
        let mut changes = router.subscribe_generation();
        router
            .handle(ResourceRequest::Signal {
                uri: uri("changed"),
                name: "refresh".into(),
                payload: None,
            })
            .await
            .unwrap();
        changes.changed().await.unwrap();

        let router = ResourceRouter::new();
        router
            .register(
                "observer",
                ResourceRoute::new(
                    "file:///**",
                    None::<String>,
                    [Read, ResourceOperation::Children, ResourceOperation::Poll],
                ),
                Arc::new(Text("ok")),
            )
            .await
            .unwrap();
        let requests = [
            ResourceRequest::Read {
                uri: uri("changed"),
                start_line: None,
                line_count: None,
            },
            ResourceRequest::Children {
                uri: uri("changed"),
            },
            ResourceRequest::Poll {
                uri: uri("changed"),
                pattern: None,
                timeout: None,
                cursor: None,
            },
        ];
        for request in requests {
            let changes = router.subscribe_generation();
            router.handle(request).await.unwrap();
            assert!(!changes.has_changed().unwrap());
        }
    }

    #[tokio::test]
    async fn signal_declarations_and_payload_schemas_are_enforced() {
        let router = ResourceRouter::new();
        router
            .register(
                "signals",
                ResourceRoute::new("file:///**", None::<String>, [ResourceOperation::Signal])
                    .with_signals(vec![
                        SignalDefinition {
                            name: "resize".into(),
                            description: "resize it".into(),
                            payload_schema: json!({
                                "type": "object",
                                "required": ["width"],
                                "properties": {"width": {"type": "integer"}}
                            }),
                        },
                        SignalDefinition {
                            name: "reset".into(),
                            description: "reset it".into(),
                            payload_schema: json!({"type": "null"}),
                        },
                    ]),
                Arc::new(Text("accepted")),
            )
            .await
            .unwrap();
        let signal = |name: &str, payload: Option<&str>| ResourceRequest::Signal {
            uri: uri("source.rs"),
            name: name.into(),
            payload: payload.map(str::to_owned),
        };
        assert!(matches!(
            router.handle(signal("missing", Some("null"))).await,
            Err(ResourceError::Invalid(message)) if message.contains("not declared")
        ));
        assert!(matches!(
            router.handle(signal("resize", Some("not-json"))).await,
            Err(ResourceError::Invalid(message)) if message.contains("valid JSON")
        ));
        assert!(matches!(
            router.handle(signal("resize", Some(r#"{"width":"wide"}"#))).await,
            Err(ResourceError::Invalid(message)) if message.contains("does not match")
        ));
        assert_eq!(
            router
                .handle(signal("resize", Some(r#"{"width":80}"#)))
                .await
                .unwrap(),
            ResourceReply::Text {
                text: "accepted".into()
            }
        );
        assert!(router.handle(signal("reset", None)).await.is_ok());

        let empty = ResourceRouter::new();
        empty
            .register(
                "empty",
                ResourceRoute::new("file:///**", None::<String>, [ResourceOperation::Signal]),
                Arc::new(Text("unreachable")),
            )
            .await
            .unwrap();
        assert!(matches!(
            empty.handle(signal("anything", None)).await,
            Err(ResourceError::Invalid(message)) if message.contains("not declared")
        ));
    }

    #[tokio::test]
    async fn kernel_synthesizes_stable_metadata() {
        let router = ResourceRouter::new();
        let provider = Arc::new(Text("unused"));
        router
            .register(
                "read",
                ResourceRoute::new("file:///**", None::<String>, [Read]),
                provider.clone(),
            )
            .await
            .unwrap();
        router
            .register(
                "signals",
                ResourceRoute::new("file:///**", None::<String>, [ResourceOperation::Signal])
                    .with_signals(vec![
                        SignalDefinition {
                            name: "zeta".into(),
                            description: "z".into(),
                            payload_schema: json!({}),
                        },
                        SignalDefinition {
                            name: "alpha".into(),
                            description: "a".into(),
                            payload_schema: json!({"type": "string"}),
                        },
                    ]),
                provider,
            )
            .await
            .unwrap();

        let metadata = router.metadata(&uri("source.rs")).unwrap();
        assert_eq!(metadata.operations, [Read, ResourceOperation::Signal]);
        assert_eq!(metadata.signals[0].name, "alpha");
        assert_eq!(metadata.signals[1].name, "zeta");

        let reply = router
            .handle(ResourceRequest::Read {
                uri: uri("source.rs?meta"),
                start_line: None,
                line_count: None,
            })
            .await
            .unwrap();
        let ResourceReply::Text { text } = reply else {
            panic!("metadata must be text")
        };
        assert_eq!(
            serde_json::from_str::<ResourceMetadata>(&text).unwrap(),
            metadata
        );
        assert!(!text.contains("\"plugin\""));
    }

    #[tokio::test]
    async fn metadata_can_describe_a_projection() {
        let router = ResourceRouter::new();
        router
            .register(
                "symbols",
                ResourceRoute::new("file:///**", Some("symbols/**"), [Read]),
                Arc::new(Text("symbol")),
            )
            .await
            .unwrap();
        let reply = router
            .handle(ResourceRequest::Read {
                uri: uri("source.rs?meta/symbols/foo"),
                start_line: None,
                line_count: None,
            })
            .await
            .unwrap();
        let ResourceReply::Text { text } = reply else {
            panic!("metadata must be text")
        };
        let metadata: ResourceMetadata = serde_json::from_str(&text).unwrap();
        assert_eq!(metadata.uri, uri("source.rs?symbols/foo"));
        assert_eq!(metadata.operations, [Read]);
    }

    #[tokio::test]
    async fn providers_cannot_claim_the_meta_projection() {
        let router = ResourceRouter::new();
        let result = router
            .register(
                "bad",
                ResourceRoute::new("file:///**", Some("meta/**"), [Read]),
                Arc::new(Text("bad")),
            )
            .await;
        assert!(matches!(result, Err(ResourceError::Invalid(_))));
    }

    #[tokio::test]
    async fn owner_replacement_accepts_multiple_routes_and_operations_atomically() {
        let router = ResourceRouter::new();
        let old: Arc<dyn ResourceProvider> = Arc::new(Text("old"));
        router
            .register(
                "owner",
                ResourceRoute::new("file:///**", None::<String>, [Read]),
                old,
            )
            .await
            .unwrap();
        let provider: Arc<dyn ResourceProvider> = Arc::new(Text("new"));
        router
            .replace_owner(
                "owner",
                vec![
                    (
                        ResourceRoute::new(
                            "file:///work/**",
                            None::<String>,
                            [Read, ResourceOperation::Write],
                        ),
                        provider.clone(),
                    ),
                    (
                        ResourceRoute::new("mem:///**", None::<String>, [Read]),
                        provider.clone(),
                    ),
                ],
            )
            .unwrap();
        assert_eq!(router.routes().len(), 2);
        assert_eq!(
            router.route_owner(&uri("source.rs"), ResourceOperation::Write),
            Some("owner".into())
        );

        let result = router.replace_owner(
            "owner",
            vec![
                (
                    ResourceRoute::new("file:///**", None::<String>, [Read]),
                    provider.clone(),
                ),
                (
                    ResourceRoute::new("[invalid", None::<String>, [Read]),
                    provider,
                ),
            ],
        );
        assert!(result.is_err());
        assert_eq!(router.routes().len(), 2);
        assert_eq!(
            router.route_owner(&uri("source.rs"), ResourceOperation::Write),
            Some("owner".into())
        );
    }

    #[tokio::test]
    async fn run_and_signal_are_forwarded_without_interpretation() {
        struct Control;
        #[async_trait]
        impl ResourceProvider for Control {
            async fn handle(
                &self,
                request: ResourceRequest,
            ) -> Result<ResourceReply, ResourceError> {
                match request {
                    ResourceRequest::Run {
                        target,
                        input,
                        cwd,
                        env,
                        timeout,
                    } => {
                        assert_eq!(target, uri("control"));
                        assert_eq!(input, "payload");
                        assert_eq!(cwd, Some(uri("cwd")));
                        assert_eq!(
                            env,
                            vec![EnvironmentEntry {
                                name: "KEY".into(),
                                value: "value".into()
                            }]
                        );
                        assert_eq!(timeout, Some(std::time::Duration::from_millis(25)));
                        Ok(ResourceReply::Started {
                            uri: uri("work/created"),
                        })
                    }
                    ResourceRequest::Signal {
                        uri: target,
                        name,
                        payload,
                    } => {
                        assert_eq!(target, uri("control"));
                        assert_eq!(name, "pause");
                        assert_eq!(payload.as_deref(), Some("\"because\""));
                        Ok(ResourceReply::Signaled)
                    }
                    other => Err(ResourceError::Unsupported {
                        uri: other.uri().clone(),
                        operation: other.operation(),
                    }),
                }
            }
        }

        let router = ResourceRouter::new();
        router
            .register(
                "control",
                ResourceRoute::new(
                    "file:///**",
                    None::<String>,
                    [ResourceOperation::Run, ResourceOperation::Signal],
                )
                .with_signals(vec![SignalDefinition {
                    name: "pause".into(),
                    description: "pause execution".into(),
                    payload_schema: json!({"type": "string"}),
                }]),
                Arc::new(Control),
            )
            .await
            .unwrap();
        assert_eq!(
            router
                .handle(ResourceRequest::Run {
                    target: uri("control"),
                    input: "payload".into(),
                    cwd: Some(uri("cwd")),
                    env: vec![EnvironmentEntry {
                        name: "KEY".into(),
                        value: "value".into(),
                    }],
                    timeout: Some(std::time::Duration::from_millis(25)),
                })
                .await
                .unwrap(),
            ResourceReply::Started {
                uri: uri("work/created")
            }
        );
        assert_eq!(
            router
                .handle(ResourceRequest::Signal {
                    uri: uri("control"),
                    name: "pause".into(),
                    payload: Some("\"because\"".into()),
                })
                .await
                .unwrap(),
            ResourceReply::Signaled
        );
    }

    #[tokio::test]
    async fn validates_execution_neutral_control_requests() {
        let router = ResourceRouter::new();
        assert!(matches!(
            router
                .handle(ResourceRequest::Run {
                    target: uri("control"),
                    input: String::new(),
                    cwd: None,
                    env: Vec::new(),
                    timeout: Some(std::time::Duration::ZERO),
                })
                .await,
            Err(ResourceError::Invalid(_))
        ));
        assert!(matches!(
            router
                .handle(ResourceRequest::Signal {
                    uri: uri("control"),
                    name: String::new(),
                    payload: None,
                })
                .await,
            Err(ResourceError::Invalid(_))
        ));
    }
}
