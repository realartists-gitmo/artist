use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use globset::{Glob, GlobMatcher};
use tokio::sync::{RwLock, watch};

use crate::{
    ResourceError, ResourceOperation, ResourceProvider, ResourceReply, ResourceRequest,
    ResourceRoute, ResourceUri,
};

#[derive(Clone)]
pub struct ResourceRouter {
    inner: Arc<RouterInner>,
}

struct RouterInner {
    routes: RwLock<Vec<RegisteredRoute>>,
    generation: AtomicU64,
    topology: watch::Sender<u64>,
}

impl Default for ResourceRouter {
    fn default() -> Self {
        let (topology, _) = watch::channel(0);
        Self {
            inner: Arc::new(RouterInner {
                routes: RwLock::new(Vec::new()),
                generation: AtomicU64::new(0),
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
        self.inner.generation.load(Ordering::Acquire)
    }

    /// Observe route registrations and successful topology-changing requests.
    pub fn subscribe_generation(&self) -> watch::Receiver<u64> {
        self.inner.topology.subscribe()
    }

    fn changed(&self) {
        let generation = self.inner.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.inner.topology.send_replace(generation);
    }

    pub async fn register(
        &self,
        plugin: impl Into<String>,
        route: ResourceRoute,
        provider: Arc<dyn ResourceProvider>,
    ) -> Result<(), ResourceError> {
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
        let mut routes = self.inner.routes.write().await;
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

    pub async fn route_owner(
        &self,
        uri: &ResourceUri,
        operation: ResourceOperation,
    ) -> Option<String> {
        self.select(uri, operation)
            .await
            .map(|route| route.plugin.clone())
    }

    pub async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let operation = request.operation();
        let uri = request.uri().clone();
        let route = match self.select(&uri, operation).await {
            Some(route) => route,
            None if self.has_route(&uri).await => {
                return Err(ResourceError::Unsupported { uri, operation });
            }
            None => return Err(ResourceError::NotFound { uri, operation }),
        };
        if let ResourceRequest::Move { to: Some(to), .. } = &request {
            let destination = self.select(to, operation).await;
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
        let result = route.provider.handle(request).await;
        if result.is_ok()
            && matches!(
                operation,
                ResourceOperation::Write | ResourceOperation::Move
            )
        {
            self.changed();
        }
        result
    }

    async fn select(
        &self,
        uri: &ResourceUri,
        operation: ResourceOperation,
    ) -> Option<RouteSelection> {
        let routes = self.inner.routes.read().await;
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
            })
    }

    async fn has_route(&self, uri: &ResourceUri) -> bool {
        self.inner
            .routes
            .read()
            .await
            .iter()
            .any(|route| route.matches_node(uri))
    }

    pub async fn routes(&self) -> Vec<(String, ResourceRoute)> {
        self.inner
            .routes
            .read()
            .await
            .iter()
            .map(|r| (r.plugin.clone(), r.declaration.clone()))
            .collect()
    }

    pub async fn schemes(&self) -> Vec<String> {
        let mut schemes = self
            .inner
            .routes
            .read()
            .await
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
    pub async fn projection_roots(&self, base: &ResourceUri) -> Vec<String> {
        let routes = self.inner.routes.read().await;
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

struct RouteSelection {
    plugin: String,
    provider: Arc<dyn ResourceProvider>,
}
impl RegisteredRoute {
    fn matches_node(&self, uri: &ResourceUri) -> bool {
        matches_uri(&self.base, self.projection.as_ref(), uri)
            || self.base.is_match(uri.base().to_string())
                && self
                    .declaration
                    .projection_glob
                    .as_deref()
                    .and_then(|glob| glob.split('/').next())
                    .is_some_and(|root| uri.projection_segments() == [root])
    }

    fn matches(&self, uri: &ResourceUri, operation: ResourceOperation) -> bool {
        matches_uri(&self.base, self.projection.as_ref(), uri)
            || operation == ResourceOperation::Children
                && self.base.is_match(uri.base().to_string())
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
    use crate::{ResourceOperation::Read, ResourceReply};
    use async_trait::async_trait;

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
        assert_eq!(
            tied.route_owner(&uri("x"), Read).await.as_deref(),
            Some("first")
        );
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
                .await
                .as_deref(),
            Some("ast")
        );
        assert_eq!(
            router
                .route_owner(&uri("src/lib.rs"), Read)
                .await
                .as_deref(),
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
    async fn publishes_generation_changes_for_routes_and_mutations() {
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
}
