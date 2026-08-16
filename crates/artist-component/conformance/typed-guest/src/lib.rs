use artist::resource::types;

struct TypedTool;

fn error(message: impl ToString, uri: Option<String>) -> types::Error {
    types::Error {
        code: types::ErrorCode::Internal,
        uri,
        message: message.to_string(),
    }
}

fn observe<T: serde::Serialize>(response: Result<T, types::Error>) -> String {
    serde_json::to_string(&response)
        .unwrap_or_else(|error| format!("observer serialization error: {error}"))
}

fn invoke_json(verb: &str, uri: &str, input: serde_json::Value) -> Result<serde_json::Value, types::Error> {
    let output = artist::resource::host::invoke(verb, uri, &input.to_string());
    let value = serde_json::from_str::<serde_json::Value>(&output)
        .map_err(|parse_error| error(format!("invalid {verb} host output: {parse_error}"), Some(uri.to_owned())))?;
    if let Some(error_value) = value.get("err") {
        return Err(json_error(error_value, uri));
    }
    value.get("ok").cloned().ok_or_else(|| {
        error(format!("{verb} host output has neither ok nor err"), Some(uri.to_owned()))
    })
}

fn invoke_json_batch(
    verb: &str,
    requests: Vec<(String, serde_json::Value)>,
) -> Vec<Result<serde_json::Value, types::Error>> {
    let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
    let inputs = requests.iter().map(|(_, input)| input.to_string()).collect::<Vec<_>>();
    let outputs = artist::resource::host::invoke_batch(verb, &uris, &inputs);
    uris.into_iter().zip(outputs).map(|(uri, output)| {
        let value = serde_json::from_str::<serde_json::Value>(&output)
            .map_err(|parse_error| error(parse_error.to_string(), Some(uri.clone())))?;
        if let Some(error_value) = value.get("err") {
            return Err(json_error(error_value, &uri));
        }
        value.get("ok").cloned().ok_or_else(|| {
            error("resource host batch output has neither ok nor err", Some(uri))
        })
    }).collect()
}

#[cfg(any(feature = "read", feature = "poll"))]
fn position_json(position: Option<&types::Position>) -> serde_json::Value {
    match position {
        None => serde_json::Value::Null,
        Some(types::Position::Top) => serde_json::json!("top"),
        Some(types::Position::Bottom) => serde_json::json!("bottom"),
        Some(types::Position::At(anchor)) => serde_json::json!(anchor),
    }
}

#[cfg(feature = "read")]
fn text(uri: String, content: &str) -> types::AnchoredText {
    types::AnchoredText {
        uri,
        lines: content
            .lines()
            .enumerate()
            .map(|(index, value)| types::AnchoredLine {
                anchor: format!("line-{}", index + 1),
                text: value.to_owned(),
                ending: types::LineEnding::Lf,
            })
            .collect(),
    }
}

#[cfg(feature = "read")]
impl exports::artist::tool::read::Guest for TypedTool {
    fn read(
        requests: Vec<exports::artist::tool::read::ReadRequest>,
    ) -> Vec<Result<exports::artist::tool::read::ReadResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let uri = request.uri.clone();
            (uri.clone(), serde_json::json!({
                    "uri": uri,
                    "at": position_json(request.at.as_ref()),
                    "before": request.before,
                    "after": request.after,
                }))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("read", requests).into_iter().zip(uris).map(|(output, uri)| {
            output.and_then(|value| read_response(&serde_json::json!({"ok": value}).to_string(), &uri))
        }).collect()
    }

    fn observe(response: Result<exports::artist::tool::read::ReadResponse, types::Error>) -> String {
        observe(response)
    }
}

#[cfg(feature = "read")]
fn read_response(
    output: &str,
    fallback_uri: &str,
) -> Result<exports::artist::tool::read::ReadResponse, types::Error> {
    let value: serde_json::Value = serde_json::from_str(output).map_err(|parse_error| {
        error(
            format!("invalid read host output: {parse_error}"),
            Some(fallback_uri.to_owned()),
        )
    })?;
    if let Some(error_value) = value.get("err") {
        return Err(json_error(error_value, fallback_uri));
    }
    let success = value.get("ok").ok_or_else(|| {
        error("read host output has neither ok nor err".to_owned(), Some(fallback_uri.to_owned()))
    })?;
    if let Some(lines) = success.get("lines") {
        let text_value = if lines.get("lines").is_some() {
            lines
        } else {
            success
        };
        return Ok(exports::artist::tool::read::ReadResponse::Text(json_text(
            text_value,
            fallback_uri,
        )?));
    }
    if let Some(text_value) = success.get("text") {
        return Ok(exports::artist::tool::read::ReadResponse::Text(json_text(
            text_value,
            fallback_uri,
        )?));
    }
    if let Some(directory) = success.get("directory") {
        let uri = directory
            .get("uri")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback_uri)
            .to_owned();
        let entries = directory
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| error("read directory has no entries".to_owned(), Some(uri.clone())))?
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| error("read directory entry is not a URI".to_owned(), Some(uri.clone())))
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(exports::artist::tool::read::ReadResponse::Directory(
            exports::artist::tool::read::DirectoryResult { uri, entries },
        ));
    }
    if let Some(entries_value) = success.get("entries") {
        let directory = if entries_value.get("entries").is_some() {
            entries_value
        } else {
            success
        };
        let uri = directory
            .get("uri")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback_uri)
            .to_owned();
        let entries = directory
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| error("read directory has no entries", Some(uri.clone())))?
            .iter()
            .map(|entry| entry.as_str().map(str::to_owned).ok_or_else(|| error("read directory entry is not a URI", Some(uri.clone()))))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(exports::artist::tool::read::ReadResponse::Directory(
            exports::artist::tool::read::DirectoryResult { uri, entries },
        ));
    }
    // Execution providers such as osproc and invocation channels expose
    // their textual state under provider-specific field names. The universal
    // read contract still returns anchored text; preserve that content rather
    // than rejecting an otherwise valid provider response.
    if let Some(content) = success.get("output").or_else(|| success.get("state")) {
        let content_text = content
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| content.to_string());
        return Ok(exports::artist::tool::read::ReadResponse::Text(text(
            fallback_uri.to_owned(),
            &content_text,
        )));
    }
    Err(error(
        format!("read host output has unknown response variant: {success}"),
        Some(fallback_uri.to_owned()),
    ))
}

#[cfg(any(feature = "read", feature = "write", feature = "edit", feature = "insert", feature = "grep", feature = "poll"))]
fn json_text(
    value: &serde_json::Value,
    fallback_uri: &str,
) -> Result<types::AnchoredText, types::Error> {
    let uri = value
        .get("uri")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(fallback_uri)
        .to_owned();
    let lines = value
        .get("lines")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| error("read text has no lines".to_owned(), Some(uri.clone())))?
        .iter()
        .map(|line| {
            let ending = match line.get("ending").and_then(serde_json::Value::as_str) {
                Some("crlf") => types::LineEnding::Crlf,
                Some("cr") => types::LineEnding::Cr,
                Some("none") => types::LineEnding::None,
                _ => types::LineEnding::Lf,
            };
            Ok(types::AnchoredLine {
                anchor: match line.get("anchor") {
                    Some(serde_json::Value::String(anchor)) => anchor.clone(),
                    Some(serde_json::Value::Array(tokens)) => format!(
                        "#{}",
                        tokens.iter().filter_map(serde_json::Value::as_str).collect::<Vec<_>>().join(".")
                    ),
                    _ => String::new(),
                },
                text: line.get("text").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned(),
                ending,
            })
        })
        .collect::<Result<Vec<_>, types::Error>>()?;
    Ok(types::AnchoredText { uri, lines })
}

#[cfg(any(feature = "edit", feature = "insert"))]
fn json_diff(value: &serde_json::Value, fallback_uri: &str) -> Result<types::AnchoredDiff, types::Error> {
    let uri = value.get("uri").and_then(serde_json::Value::as_str).unwrap_or(fallback_uri).to_owned();
    let hunks = value.get("hunks").and_then(serde_json::Value::as_array).ok_or_else(|| error("diff has no hunks", Some(uri.clone())))?
        .iter().map(|hunk| {
            let old = hunk.get("old").and_then(serde_json::Value::as_array).ok_or_else(|| error("diff hunk has no old lines", Some(uri.clone())))?
                .iter().map(|line| json_line(line, &uri)).collect::<Result<Vec<_>, _>>()?;
            let new = hunk.get("new").and_then(serde_json::Value::as_array).ok_or_else(|| error("diff hunk has no new lines", Some(uri.clone())))?
                .iter().map(|line| json_line(line, &uri)).collect::<Result<Vec<_>, _>>()?;
            Ok(types::DiffHunk { old, new })
        }).collect::<Result<Vec<_>, types::Error>>()?;
    Ok(types::AnchoredDiff { uri, hunks })
}

#[cfg(any(feature = "edit", feature = "insert", feature = "grep", feature = "poll"))]
fn json_line(value: &serde_json::Value, fallback_uri: &str) -> Result<types::AnchoredLine, types::Error> {
    let text = value.get("text").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let ending = match value.get("ending").and_then(serde_json::Value::as_str) {
        Some("crlf") => types::LineEnding::Crlf,
        Some("cr") => types::LineEnding::Cr,
        Some("none") => types::LineEnding::None,
        _ => types::LineEnding::Lf,
    };
    let anchor = match value.get("anchor") {
        Some(serde_json::Value::String(anchor)) => anchor.clone(),
        Some(serde_json::Value::Array(tokens)) => format!("#{}", tokens.iter().filter_map(serde_json::Value::as_str).collect::<Vec<_>>().join(".")),
        _ => return Err(error("line has no anchor", Some(fallback_uri.to_owned()))),
    };
    Ok(types::AnchoredLine { anchor, text, ending })
}

#[cfg(any(feature = "edit", feature = "insert"))]
fn json_changed(value: &serde_json::Value, fallback_uri: &str) -> Result<Vec<types::AnchoredText>, types::Error> {
    value.get("changed").and_then(serde_json::Value::as_array).ok_or_else(|| error("response has no changed text", Some(fallback_uri.to_owned())))?
        .iter().map(|value| json_text(value, fallback_uri)).collect()
}

fn json_error(value: &serde_json::Value, fallback_uri: &str) -> types::Error {
    let code = match value.get("code").and_then(serde_json::Value::as_str) {
        Some("invalid-uri") => types::ErrorCode::InvalidUri,
        Some("invalid-input") => types::ErrorCode::InvalidInput,
        Some("invalid-pattern") => types::ErrorCode::InvalidPattern,
        Some("not-found") => types::ErrorCode::NotFound,
        Some("wrong-kind") => types::ErrorCode::WrongKind,
        Some("invalid-anchor") => types::ErrorCode::InvalidAnchor,
        Some("stale-anchor") => types::ErrorCode::StaleAnchor,
        Some("immutable") => types::ErrorCode::Immutable,
        Some("permission-denied") => types::ErrorCode::PermissionDenied,
        Some("conflict") => types::ErrorCode::Conflict,
        Some("not-empty") => types::ErrorCode::NotEmpty,
        Some("aborted") => types::ErrorCode::Aborted,
        Some("unsupported") => types::ErrorCode::Unsupported,
        _ => types::ErrorCode::Internal,
    };
    types::Error {
        code,
        uri: value
            .get("uri")
            .and_then(serde_json::Value::as_str)
            .or(Some(fallback_uri))
            .map(str::to_owned),
        message: value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("resource invocation failed")
            .to_owned(),
    }
}

#[cfg(feature = "write")]
impl exports::artist::tool::write::Guest for TypedTool {
    fn write(
        requests: Vec<exports::artist::tool::write::WriteRequest>,
    ) -> Vec<Result<exports::artist::tool::write::WriteResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let uri = request.uri.clone();
            (uri.clone(), serde_json::json!({"uri": uri, "content": request.content}))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("write", requests).into_iter().zip(uris).map(|(output, uri)| output.and_then(|output| {
                let text = output
                    .get("text")
                    .filter(|value| !value.is_null())
                    .map(|value| json_text(value, &uri))
                    .transpose()?;
                Ok(exports::artist::tool::write::WriteResponse {
                    uri,
                    text,
                })
        })).collect()
    }

    fn observe(response: Result<exports::artist::tool::write::WriteResponse, types::Error>) -> String {
        super::observe(response)
    }
}

#[cfg(feature = "edit")]
impl exports::artist::tool::edit::Guest for TypedTool {
    fn edit(
        requests: Vec<exports::artist::tool::edit::EditRequest>,
    ) -> Vec<Result<exports::artist::tool::edit::EditResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let uri = request.uri.clone();
            (uri.clone(), serde_json::json!({
                    "uri": uri.clone(),
                    "start": request.start,
                    "end": request.end,
                    "content": request.content,
                }))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("edit", requests)
            .into_iter()
            .zip(uris)
            .map(|(output, uri)| {
                output.and_then(|output| {
                    Ok(exports::artist::tool::edit::EditResponse {
                        uri: uri.clone(),
                        changed: json_changed(&output, &uri)?,
                        diff: json_diff(
                            output.get("diff").ok_or_else(|| {
                                error("edit response has no diff", Some(uri.clone()))
                            })?,
                            &uri,
                        )?,
                    })
                })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::edit::EditResponse, types::Error>) -> String {
        super::observe(response)
    }
}

#[cfg(feature = "insert")]
impl exports::artist::tool::insert::Guest for TypedTool {
    fn insert(
        requests: Vec<exports::artist::tool::insert::InsertRequest>,
    ) -> Vec<Result<exports::artist::tool::insert::InsertResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let uri = request.uri.clone();
            (uri.clone(), serde_json::json!({
                    "uri": uri.clone(),
                    "at": match request.at {
                        exports::artist::tool::insert::InsertionPosition::Top => serde_json::json!({"top": null}),
                        exports::artist::tool::insert::InsertionPosition::Bottom => serde_json::json!({"bottom": null}),
                        exports::artist::tool::insert::InsertionPosition::At(anchor) => serde_json::json!({"at": anchor}),
                    },
                    "content": request.content,
                }))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("insert", requests).into_iter().zip(uris).map(|(output, uri)| { output.and_then(|output| {
                Ok(exports::artist::tool::insert::InsertResponse {
                    uri: uri.clone(),
                    changed: json_changed(&output, &uri)?,
                    diff: json_diff(output.get("diff").ok_or_else(|| error("insert response has no diff", Some(uri.clone())))?, &uri)?,
                })
            })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::insert::InsertResponse, types::Error>) -> String {
        super::observe(response)
    }
}

#[cfg(feature = "find")]
impl exports::artist::tool::find::Guest for TypedTool {
    fn find(
        requests: Vec<exports::artist::tool::find::FindRequest>,
    ) -> Vec<Result<exports::artist::tool::find::FindResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let root = request.root.clone();
            (root.clone(), serde_json::json!({
                    "root": root,
                    "query": request.query,
                }))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("find", requests).into_iter().zip(uris).map(|(output, root)| { output.and_then(|output| {
                let values = output.get("uris").or_else(|| output.get("entries"))
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| error("find response has no uris", Some(root.clone())))?
                    .iter().map(|uri| uri.as_str().map(str::to_owned).ok_or_else(|| error("find response has a non-uri", Some(root.clone())))).collect::<Result<Vec<_>, _>>()?;
                Ok(exports::artist::tool::find::FindResponse { uris: values })
            })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::find::FindResponse, types::Error>) -> String {
        super::observe(response)
    }
}

#[cfg(feature = "grep")]
impl exports::artist::tool::grep::Guest for TypedTool {
    fn grep(
        requests: Vec<exports::artist::tool::grep::GrepRequest>,
    ) -> Vec<Result<exports::artist::tool::grep::GrepResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let uri = request.uri.clone();
            (uri.clone(), serde_json::json!({
                    "uri": uri,
                    "pattern": request.pattern,
                }))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("grep", requests).into_iter().zip(uris).map(|(output, uri)| { output.and_then(|output| {
                Ok(exports::artist::tool::grep::GrepResponse {
                    matches: output.get("matches").and_then(serde_json::Value::as_array)
                        .ok_or_else(|| error("grep response has no matches", Some(uri.clone())))?
                        .iter().map(|value| json_text(value, &uri)).collect::<Result<Vec<_>, _>>()?,
                })
            })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::grep::GrepResponse, types::Error>) -> String {
        super::observe(response)
    }
}

#[cfg(feature = "run")]
impl exports::artist::tool::run::Guest for TypedTool {
    fn run(
        requests: Vec<exports::artist::tool::run::RunRequest>,
    ) -> Vec<Result<exports::artist::tool::run::RunResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let uri = request.uri.clone();
            (uri.clone(), serde_json::json!({"uri": uri, "args": request.args}))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("run", requests).into_iter().zip(uris).map(|(output, uri)| output.and_then(|output| {
                let execution_uri = output.get("uri").and_then(serde_json::Value::as_str).ok_or_else(|| error("run response has no execution uri", Some(uri.clone())))?;
                Ok(exports::artist::tool::run::RunResponse { uri: execution_uri.to_owned() })
            })).collect()
    }

    fn observe(response: Result<exports::artist::tool::run::RunResponse, types::Error>) -> String {
        super::observe(response)
    }
}

#[cfg(feature = "poll")]
impl exports::artist::tool::poll::Guest for TypedTool {
    fn poll(
        requests: Vec<exports::artist::tool::poll::PollRequest>,
    ) -> Vec<Result<exports::artist::tool::poll::PollResponse, types::Error>> {
        let requests = requests.into_iter().map(|request| {
            let uri = request.uri.clone();
            (uri.clone(), serde_json::json!({
                    "uri": uri,
                    "from": position_json(request.from.as_ref()),
                    "match": request.match,
                    "timeout-ms": request.timeout_ms,
                }))
        }).collect::<Vec<_>>();
        let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
        invoke_json_batch("poll", requests).into_iter().zip(uris).map(|(output, uri)| output.and_then(|output| {
                let reason = match output.get("reason").and_then(serde_json::Value::as_str) {
                    Some("changed") => exports::artist::tool::poll::PollReason::Changed,
                    Some("matched") => exports::artist::tool::poll::PollReason::Matched,
                    Some("terminated") => exports::artist::tool::poll::PollReason::Terminated,
                    Some("timeout") => exports::artist::tool::poll::PollReason::Timeout,
                    _ => return Err(error("poll response has invalid reason", Some(uri.clone()))),
                };
                Ok(exports::artist::tool::poll::PollResponse {
                    uri: uri.clone(),
                    text: json_text(output.get("text").ok_or_else(|| error("poll response has no text", Some(uri.clone())))?, &uri)?,
                    reason,
                })
            })).collect()
    }

    fn observe(response: Result<exports::artist::tool::poll::PollResponse, types::Error>) -> String {
        super::observe(response)
    }
}

macro_rules! uri_verb {
    ($feature:literal, $module:ident) => {
        #[cfg(feature = $feature)]
        impl exports::artist::tool::$module::Guest for TypedTool {
            fn $module(
                requests: Vec<exports::artist::tool::$module::UriRequest>,
            ) -> Vec<Result<exports::artist::tool::$module::UriResponse, types::Error>> {
                let requests = requests.into_iter().map(|request| {
                    let uri = request.uri.clone();
                    (uri.clone(), serde_json::json!({"uri": uri}))
                }).collect::<Vec<_>>();
                let uris = requests.iter().map(|(uri, _)| uri.clone()).collect::<Vec<_>>();
                invoke_json_batch(stringify!($module), requests).into_iter().zip(uris).map(|(output, uri)| output.and_then(|output| {
                        let returned_uri = output.get("uri").and_then(serde_json::Value::as_str).or_else(|| output.as_str()).unwrap_or(&uri);
                        Ok(exports::artist::tool::$module::UriResponse { uri: returned_uri.to_owned() })
                    })).collect()
            }

            fn observe(response: Result<exports::artist::tool::$module::UriResponse, types::Error>) -> String {
                observe(response)
            }
        }
    };
}

uri_verb!("abort", abort);
uri_verb!("delete", delete);

export!(TypedTool);
