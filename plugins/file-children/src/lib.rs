use artist_plugin_sdk::artist::plugin::{native_filesystem, types::*};

struct FileChildren;

fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
    let ResourceRequest::Children(uri) = request else {
        return Err(unsupported());
    };
    native_filesystem::children(&uri)
        .map(ResourceReply::Children)
        .map_err(provider)
}

fn provider(message: String) -> ResourceError {
    ResourceError::Provider(message)
}

fn unsupported() -> ResourceError {
    ResourceError::Unsupported(RouteError {
        uri: "file:/".into(),
        operation: ResourceOperation::Children,
    })
}

artist_plugin_sdk::resource_component!(
    FileChildren,
    "artist.file.children",
    vec![ResourceRoute {
        base_glob: "file:///**".into(),
        projection_glob: None,
        operations: vec![ResourceOperation::Children],
        signals: Vec::new(),
    }],
    handle
);
