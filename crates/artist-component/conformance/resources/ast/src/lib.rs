#![allow(unexpected_cfgs)]

wit_bindgen::generate!({
    path: "../",
    world: "ast",
    generate_all,
});

use artist::resource::types;
// Keep the generated bindings refreshed when package-local WIT dependencies change.
use artist::resource::read as source_read;

struct AstResource;

fn error(message: impl ToString, uri: Option<String>) -> types::Error {
    types::Error {
        code: types::ErrorCode::Internal,
        uri,
        message: message.to_string(),
    }
}

fn source_uri(uri: &str) -> Option<String> {
    let path = uri.split('?').next().unwrap_or(uri);
    path.rmatch_indices("/symbols")
        .filter_map(|(index, _)| {
            let source = &path[..index];
            let suffix = &path[index + "/symbols".len()..];
            if !source.starts_with("file://")
                || !suffix
                    .chars()
                    .next()
                    .is_none_or(|character| character == '/')
            {
                return None;
            }
            Some(source.to_owned())
        })
        .next()
}

impl exports::artist::resource::extension::Guest for AstResource {
    fn claim(request: types::ClaimRequest) -> types::ClaimDecision {
        // This component is a proof only. The application disables its file
        // route, leaving native artist_ast as the production owner.
        // The production AST provider owns file projections. Keep this
        // optional package available for experimentation without stealing
        // the native route during startup.
        if request.uri.contains("/symbols") {
            types::ClaimDecision::Handle
        } else {
            types::ClaimDecision::Pass
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
                    uri: source.clone(),
                    at: None,
                    before: None,
                    after: None,
                };
                let source_text = source_read::read(&vec![source_request])
                    .into_iter()
                    .next()
                    .ok_or_else(|| error("source read returned no result", Some(target.clone())))??;
                let types::ReadResult::Text(source_text) = source_text else {
                    return Err(error("AST source is not text", Some(target)));
                };
                let path = target.split('?').next().unwrap_or(&target);
                let suffix = path
                    .strip_prefix(&source)
                    .and_then(|rest| rest.strip_prefix("/symbols"))
                    .unwrap_or_default()
                    .trim_start_matches('/');
                let symbol = suffix.split('/').next().filter(|value| !value.is_empty());
                let is_callers = suffix.split('/').any(|part| part == "callers");
                let limit = target
                    .split_once('?')
                    .and_then(|(_, query)| query.split('&').find_map(|part| part.strip_prefix("limit=")))
                    .and_then(|value| value.parse::<usize>().ok());
                let lines = source_text
                    .lines
                    .into_iter()
                    .filter(|line| {
                        if let Some(symbol) = symbol {
                            if is_callers {
                                line.text.contains(&format!("{symbol}("))
                                    && !line.text.contains(&format!("fn {symbol}"))
                            } else {
                                line.text.contains(&format!("fn {symbol}"))
                                    || line.text.contains(&format!("struct {symbol}"))
                                    || line.text.contains(&format!("enum {symbol}"))
                                    || line.text.contains(&format!("class {symbol}"))
                            }
                        } else {
                            line.text.contains("fn ")
                                || line.text.contains("struct ")
                                || line.text.contains("enum ")
                                || line.text.contains("class ")
                        }
                    })
                    .take(limit.unwrap_or(usize::MAX))
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
