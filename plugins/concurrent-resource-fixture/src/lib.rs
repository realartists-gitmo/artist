use artist_plugin_sdk::artist::plugin::{host_resources, types::*};

struct ConcurrentResource;

fn handle(request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
    let ResourceRequest::Read(_) = request else {
        return Err(ResourceError::Unsupported(RouteError {
            uri: "concurrent:/".into(),
            operation: ResourceOperation::Read,
        }));
    };
    host_resources::handle(&ResourceRequest::Read(ReadRequest {
        uri: "barrier:///wait".into(),
        start_line: None,
        line_count: None,
    }))?;
    loop {
        let expected =
            artist_plugin_sdk::provider_state_get("count").map_err(ResourceError::Provider)?;
        let current = expected
            .as_deref()
            .unwrap_or("0")
            .parse::<u64>()
            .map_err(|error| ResourceError::Provider(error.to_string()))?;
        let next = (current + 1).to_string();
        if artist_plugin_sdk::provider_state_compare_and_swap(
            "count",
            expected.as_deref(),
            Some(&next),
        )
        .map_err(ResourceError::Provider)?
        {
            return Ok(ResourceReply::Text(next));
        }
    }
}

artist_plugin_sdk::resource_component!(
    ConcurrentResource,
    "artist.test.concurrent-resource",
    vec![
        ResourceRoute {
            base_glob: "concurrent:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Read, ResourceOperation::Children],
            signals: Vec::new(),
        },
        ResourceRoute {
            base_glob: "parallel:///**".into(),
            projection_glob: None,
            operations: vec![ResourceOperation::Read],
            signals: Vec::new(),
        },
    ],
    handle
);
