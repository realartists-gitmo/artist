use artist_kernel::{
    ClaimDecision, DynamicClaimProvider, DynamicResourceProvider, DynamicRouteExtractor,
    DynamicType, DynamicValue, DynamicVerbResult, FileHandler, FileResourceProvider,
    FileVerbBindings, Kernel, RepositoryHandler, RepositoryResourceProvider,
    RepositoryVerbBindings, ResourceFuture, ResourceUri, SessionHandler, SessionResourceProvider,
    SessionVerbBindings, VerbDefinition, VerbId,
};
use std::{collections::BTreeMap, fs, sync::Arc};

struct UriRoute;

impl DynamicRouteExtractor for UriRoute {
    fn extract(
        &self,
        input: &DynamicValue,
    ) -> Result<Vec<ResourceUri>, artist_kernel::KernelError> {
        let DynamicValue::String(uri) = input else {
            return Err(artist_kernel::KernelError::InvalidRequest {
                message: "expected a URI string".into(),
            });
        };
        Ok(vec![ResourceUri::parse(uri)?])
    }
}

struct Resource {
    identity: VerbId,
}

struct RecordUriRoute;

impl DynamicRouteExtractor for RecordUriRoute {
    fn extract(
        &self,
        input: &DynamicValue,
    ) -> Result<Vec<ResourceUri>, artist_kernel::KernelError> {
        let DynamicValue::Record(fields) = input else {
            return Err(artist_kernel::KernelError::InvalidRequest {
                message: "expected a record".into(),
            });
        };
        let Some(DynamicValue::ResourceUri(uri)) = fields.get("uri") else {
            return Err(artist_kernel::KernelError::InvalidRequest {
                message: "expected a resource-uri field".into(),
            });
        };
        Ok(vec![uri.clone()])
    }
}

impl DynamicClaimProvider for Resource {
    fn claim(&self, verb: &VerbId, _: &ResourceUri) -> ClaimDecision {
        (verb == &self.identity)
            .then_some(ClaimDecision::Handle)
            .unwrap_or(ClaimDecision::Pass)
    }
}

impl DynamicResourceProvider for Resource {
    fn invoke<'a>(
        &'a self,
        verb: &'a VerbId,
        _: &'a ResourceUri,
        input: DynamicValue,
    ) -> ResourceFuture<'a> {
        Box::pin(async move {
            Ok(DynamicVerbResult {
                verb: verb.clone(),
                function: verb.function().to_owned(),
                output: input,
            })
        })
    }
}

#[tokio::test]
async fn dynamic_kernel_call_routes_and_pins_a_registered_resource_contract() {
    let identity = VerbId::new("example:compose/transform@1.0.0").unwrap();
    let kernel = Kernel::new();
    kernel
        .activate_verb(
            VerbDefinition::new(identity.clone(), "transform", "transform", "transform")
                .with_contract(DynamicType::String, DynamicType::String),
        )
        .unwrap();
    kernel
        .route_registry()
        .register(identity.clone(), Arc::new(UriRoute))
        .unwrap();
    kernel
        .register_dynamic_resource_provider(Arc::new(Resource {
            identity: identity.clone(),
        }))
        .unwrap();

    let results = kernel
        .handle()
        .execute_dynamic_resources(artist_kernel::DynamicVerbCall {
            verb: identity,
            function: "transform".into(),
            input: DynamicValue::String("memory://composition".into()),
        })
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].uri.to_string(), "memory://composition");
}

#[tokio::test]
async fn filesystem_provider_executes_delete_through_dynamic_resource_routing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("delete-me.txt");
    fs::write(&path, "temporary\n").unwrap();

    let identity = VerbId::new("artist:filesystem/delete@1.0.0").unwrap();
    let kernel = Kernel::new();
    kernel
        .route_registry()
        .register(identity.clone(), Arc::new(RecordUriRoute))
        .unwrap();

    let handler = Arc::new(FileHandler::new(root.path()).unwrap());
    kernel
        .register_dynamic_resource_provider(Arc::new(FileResourceProvider::new(
            handler,
            FileVerbBindings {
                read: VerbId::new("artist:filesystem/read@1.0.0").unwrap(),
                write: VerbId::new("artist:filesystem/write@1.0.0").unwrap(),
                edit: VerbId::new("artist:filesystem/edit@1.0.0").unwrap(),
                insert: VerbId::new("artist:filesystem/insert@1.0.0").unwrap(),
                delete: identity.clone(),
                find: VerbId::new("artist:filesystem/find@1.0.0").unwrap(),
                grep: VerbId::new("artist:filesystem/grep@1.0.0").unwrap(),
            },
        )))
        .unwrap();

    let uri = ResourceUri::parse(&path.display().to_string()).unwrap();
    let result = kernel
        .execute_dynamic_resources(artist_kernel::DynamicVerbCall {
            verb: identity,
            function: "delete".into(),
            input: DynamicValue::Record(BTreeMap::from([(
                "uri".into(),
                DynamicValue::ResourceUri(uri.clone()),
            )])),
        })
        .await
        .unwrap();

    assert_eq!(result[0].result.output, DynamicValue::ResourceUri(uri));
    assert!(!path.exists());
}

#[tokio::test]
async fn session_provider_executes_lifecycle_through_dynamic_resource_registry() {
    let kernel = Kernel::new();
    let write = VerbId::new("artist:session/write@1.0.0").unwrap();
    let read = VerbId::new("artist:session/read@1.0.0").unwrap();
    let poll = VerbId::new("artist:session/poll@1.0.0").unwrap();
    kernel
        .register_dynamic_resource_provider(Arc::new(SessionResourceProvider::new(
            SessionHandler::new(),
            SessionVerbBindings {
                read: read.clone(),
                write: write.clone(),
                poll: poll.clone(),
                abort: VerbId::new("artist:session/abort@1.0.0").unwrap(),
                delete: VerbId::new("artist:session/delete@1.0.0").unwrap(),
                grep: VerbId::new("artist:session/grep@1.0.0").unwrap(),
            },
        )))
        .unwrap();
    let uri = ResourceUri::parse("session://local/dynamic").unwrap();

    kernel
        .resource_registry()
        .invoke(&write, &uri, DynamicValue::Record(BTreeMap::new()))
        .await
        .unwrap();
    kernel
        .resource_registry()
        .invoke(
            &write,
            &ResourceUri::parse("session://local/dynamic/inbox").unwrap(),
            DynamicValue::Record(BTreeMap::from([(
                "content".into(),
                DynamicValue::String("hello".into()),
            )])),
        )
        .await
        .unwrap();
    let result = kernel
        .resource_registry()
        .invoke(&read, &uri, DynamicValue::Record(BTreeMap::new()))
        .await
        .unwrap();
    let DynamicValue::Record(fields) = result.output else {
        panic!("session read did not return a typed record");
    };
    assert!(fields.get("lines").is_some());
    let result = kernel
        .resource_registry()
        .invoke(&poll, &uri, DynamicValue::Record(BTreeMap::new()))
        .await
        .unwrap();
    let DynamicValue::Record(fields) = result.output else {
        panic!("session poll did not return a typed record");
    };
    assert!(fields.get("reason").is_some());
    assert!(fields.get("text").is_some());
}

#[tokio::test]
async fn repository_provider_preserves_projection_uris_through_dynamic_read() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("main.rs"), "fn main() {}\n").unwrap();
    let handler = Arc::new(RepositoryHandler::new(root.path()).unwrap());
    let project = handler.project().to_owned();
    let read = VerbId::new("artist:repository/read@1.0.0").unwrap();
    let kernel = Kernel::new();
    kernel
        .register_dynamic_resource_provider(Arc::new(RepositoryResourceProvider::new(
            handler,
            RepositoryVerbBindings {
                read: read.clone(),
                find: VerbId::new("artist:repository/find@1.0.0").unwrap(),
                grep: VerbId::new("artist:repository/grep@1.0.0").unwrap(),
            },
        )))
        .unwrap();
    let uri = ResourceUri::parse(&format!("repo://{project}/main.rs")).unwrap();
    let result = kernel
        .resource_registry()
        .invoke(&read, &uri, DynamicValue::Record(BTreeMap::new()))
        .await
        .unwrap();
    let DynamicValue::Record(fields) = result.output else {
        panic!("repository read did not return a typed record");
    };
    assert_eq!(fields.get("uri"), Some(&DynamicValue::ResourceUri(uri)));
}
