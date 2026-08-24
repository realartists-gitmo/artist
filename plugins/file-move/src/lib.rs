use artist_plugin_sdk::artist::plugin::{native_filesystem, types::*};

struct FileMove;

fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
    let ResourceRequest::Move(request) = request else {
        return Err(unsupported());
    };
    native_filesystem::move_(&request.source, request.to.as_deref())
        .map(|_| ResourceReply::Moved)
        .map_err(provider)
}

fn provider(message: String) -> ResourceError {
    ResourceError::Provider(message)
}

fn unsupported() -> ResourceError {
    ResourceError::Unsupported(RouteError {
        uri: "file:/".into(),
        operation: ResourceOperation::Move,
    })
}

artist_plugin_sdk::resource_component!(
    FileMove,
    "artist.file.move",
    vec![ResourceRoute {
        base_glob: "file:///**".into(),
        projection_glob: None,
        operations: vec![ResourceOperation::Move],
        signals: Vec::new(),
    }],
    handle
);
