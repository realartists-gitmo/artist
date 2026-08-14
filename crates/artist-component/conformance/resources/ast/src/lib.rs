#![allow(unexpected_cfgs)]

wit_bindgen::generate!({
    path: "../../../wit/resource-surface",
    world: "resource-read-world",
    generate_all,
});

use artist::resource::{self, types};

struct AstResource;

fn error(message: impl ToString, uri: Option<String>) -> types::Error {
    types::Error {
        code: types::ErrorCode::Internal,
        uri,
        message: message.to_string(),
    }
}

fn source_uri(uri: &str) -> Option<String> {
    ["/symbols", "/map", "/surface", "/show/"]
        .iter()
        .find_map(|marker| uri.find(marker).map(|index| uri[..index].to_owned()))
}

impl exports::artist::resource::extension::Guest for AstResource {
    fn claim(request: types::ClaimRequest) -> types::ClaimDecision {
        let path = request.uri.split('?').next().unwrap_or(&request.uri);
        // The application kernel's native repository projection owns the
        // complete file:// AST surface. This extension remains available for
        // explicit resource use without shadowing that implementation.
        if path.starts_with("file://") {
            return types::ClaimDecision::Pass;
        }
        let recognized = path.contains("/symbols") || source_uri(path).is_some();
        if path.contains("/symbols") && request.verb == types::Verb::Read {
            return types::ClaimDecision::Handle;
        }
        if !recognized {
            return types::ClaimDecision::Pass;
        }
        match request.verb {
            types::Verb::Read => types::ClaimDecision::Handle,
            _ => types::ClaimDecision::Reserve,
        }
    }
}

impl exports::artist::resource::read::Guest for AstResource {
    fn read(
        requests: Vec<types::ReadRequest>,
    ) -> Vec<Result<types::ReadResult, types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                let target = request.uri.clone();
                let source = source_uri(&target)
                    .ok_or_else(|| error("unsupported AST projection", Some(target.clone())))?;
                let source_request = types::ReadRequest {
                    uri: source,
                    at: None,
                    before: None,
                    after: None,
                };
                let source_text = resource::read::read(&vec![source_request])
                    .into_iter()
                    .next()
                    .ok_or_else(|| error("source read returned no result", Some(target.clone())))??;
                let types::ReadResult::Text(source_text) = source_text else {
                    return Err(error("AST source is not text", Some(target)));
                };
                let lines = source_text
                    .lines
                    .into_iter()
                    .filter(|line| {
                        line.text.contains("fn ")
                            || line.text.contains("struct ")
                            || line.text.contains("enum ")
                            || line.text.contains("class ")
                    })
                    .collect();
                Ok(types::ReadResult::Text(types::AnchoredText {
                    uri: target,
                    lines,
                }))
            })
            .collect()
    }
}

export!(AstResource);
