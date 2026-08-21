//! Rust guest implementation of the default file-shaped tools.

wit_bindgen::generate!({
    path: "../artist-kernel/wasm/verbs/wit",
    world: "tool-extension",
});

use artist_teca::{TecaLine, TecaSnapshot};
use serde::Serialize;
use serde_json::Value;

#[derive(serde::Deserialize)]
struct Envelope {
    tool: String,
    request: Value,
}

#[derive(Serialize)]
struct ReadResponse {
    uri: String,
    position: Option<String>,
    range: String,
    lines: Vec<ReadLine>,
    partial: bool,
    binary: bool,
    mime_type: Option<String>,
    resource: Option<ReadResource>,
}
#[derive(Serialize)]
struct ReadResource {
    uri: String,
    mime_type: Option<String>,
    size: usize,
}
#[derive(Clone, Serialize)]
struct ReadLine {
    anchor: String,
    content: String,
}
#[derive(Serialize)]
struct FindResponse {
    results: Vec<FindMatch>,
    truncated: bool,
}
#[derive(Serialize)]
struct FindMatch {
    uri: String,
    kind: &'static str,
}
#[derive(Serialize)]
struct GrepResponse {
    matches: Vec<GrepMatch>,
    truncated: bool,
}
#[derive(Serialize)]
struct GrepMatch {
    uri: String,
    anchor: Option<String>,
    content: String,
}

type Error = exports::artist::verbs::tool::Failure;

fn invoke(request: Vec<u8>) -> Result<Vec<u8>, Error> {
    let input: Value =
        toon_format::decode_default(std::str::from_utf8(&request).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    let envelope: Envelope = serde_json::from_value(input).map_err(|_| invalid())?;
    let response = match envelope.tool.as_str() {
        "read" => read(envelope.request)?,
        "write" => write(envelope.request)?,
        "edit" => edit(envelope.request)?,
        "move" => move_resource(envelope.request)?,
        "find" => find(envelope.request)?,
        "grep" => grep(envelope.request)?,
        _ => {
            return Err(failure(
                "unsupported",
                format!(
                    "tool {:?} is not exported by the default tool component",
                    envelope.tool
                ),
            ));
        }
    };
    toon_format::encode_default(&response)
        .map(|value| value.into_bytes())
        .map_err(|_| internal())
}

fn read(request: Value) -> Result<Value, Error> {
    let requested_uri = string(&request, "uri")?;
    let (uri, position) = split_fragment(&requested_uri);
    // TECA addresses are structural, so resolving a fragment requires a
    // complete bounded snapshot. Read in provider-sized chunks rather than
    // pretending the first 256 KiB is the whole file.
    const MAX_READ: usize = 64 * 1024 * 1024;
    let bytes = read_all_bounded(&uri, MAX_READ)?;
    let partial = false;
    let source = match std::str::from_utf8(&bytes) {
        Ok(source) => source,
        Err(_) => {
            return serde_json::to_value(ReadResponse {
                uri: canonical(&uri),
                position,
                range: request
                    .get("range")
                    .and_then(Value::as_str)
                    .unwrap_or("-200..+200")
                    .to_owned(),
                lines: Vec::new(),
                partial,
                binary: true,
                mime_type: infer_mime_type(&uri),
                resource: Some(ReadResource {
                    uri: canonical(&uri),
                    mime_type: infer_mime_type(&uri),
                    size: bytes.len(),
                }),
            })
            .map_err(|_| internal());
        }
    };
    let all: Vec<_> = source
        .lines()
        .enumerate()
        .map(|(index, content)| ReadLine {
            anchor: teca_anchor(source, index),
            content: content.to_owned(),
        })
        .collect();
    let position_index = position
        .as_deref()
        .map(|value| resolve_position(&all, value))
        .transpose()?
        .unwrap_or(0);
    let range = request
        .get("range")
        .and_then(Value::as_str)
        .unwrap_or("-200..+200");
    let (start, end) = parse_range(range)?;
    let start = (position_index as i64 + start).max(0) as usize;
    let end = (position_index as i64 + end + 1).max(0) as usize;
    let lines = all
        .get(start..end.min(all.len()))
        .unwrap_or_default()
        .to_vec();
    serde_json::to_value(ReadResponse {
        uri: canonical(&uri),
        position,
        range: range.to_owned(),
        lines,
        partial,
        binary: false,
        mime_type: infer_mime_type(&uri),
        resource: None,
    })
    .map_err(|_| internal())
}

fn write(request: Value) -> Result<Value, Error> {
    let uri = string(&request, "uri")?;
    if uri.contains('?') || uri.contains('#') {
        return Err(invalid());
    }
    let content = string(&request, "content")?;
    let existed = artist::verbs::resource_api::read(&uri, 0, 1).is_ok();
    let bytes_written =
        artist::verbs::resource_api::replace(&uri, content.as_bytes()).map_err(map_resource)?;
    Ok(serde_json::json!({
        "created": !existed,
        "replaced": existed,
        "bytes_written": bytes_written,
    }))
}

fn edit(request: Value) -> Result<Value, Error> {
    let uri = string(&request, "uri")?;
    if uri.contains('?') || uri.contains('#') {
        return Err(invalid());
    }
    let mut source = String::from_utf8(read_all(&uri)?).map_err(|_| invalid())?;
    let changes = request
        .get("changes")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let snapshot = teca_lines(&source);
    let mut resolved = Vec::with_capacity(changes.len());
    for change in changes {
        let anchor = string(change, "anchor")?;
        let replacement = change
            .get("content")
            .or_else(|| change.get("replacement"))
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let line = snapshot
            .iter()
            .find(|line| line.anchor == anchor)
            .ok_or_else(|| {
                failure(
                    "conflict",
                    format!("TECA address {anchor:?} is stale or ambiguous"),
                )
            })?;
        resolved.push((line.start, line.end, replacement.to_owned()));
    }
    resolved.sort_by_key(|(start, _, _)| *start);
    if resolved.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(failure(
            "conflict",
            "edit changes overlap in the immutable source snapshot",
        ));
    }
    for (start, end, replacement) in resolved.into_iter().rev() {
        source.replace_range(start..end, &replacement);
    }
    artist::verbs::resource_api::replace(&uri, source.as_bytes()).map_err(map_resource)?;
    Ok(serde_json::json!({
        "uri": canonical(&uri),
        "updated_anchors": teca_lines(&source).into_iter().map(|line| line.anchor).collect::<Vec<_>>(),
    }))
}

fn split_fragment(uri: &str) -> (String, Option<String>) {
    uri.split_once('#')
        .map(|(base, fragment)| (base.to_owned(), Some(fragment.to_owned())))
        .unwrap_or_else(|| (uri.to_owned(), None))
}

fn resolve_position(lines: &[ReadLine], position: &str) -> Result<usize, Error> {
    let (anchor, offset) = if let Some((anchor, suffix)) = position.rsplit_once('+') {
        (anchor, suffix.parse::<i64>().map_err(|_| invalid())?)
    } else if let Some((anchor, suffix)) = position
        .rsplit_once('-')
        .filter(|(_, suffix)| !suffix.is_empty())
    {
        (anchor, -suffix.parse::<i64>().map_err(|_| invalid())?)
    } else {
        (position, 0)
    };
    let base = lines
        .iter()
        .position(|line| line.anchor == anchor)
        .or_else(|| {
            anchor
                .parse::<usize>()
                .ok()
                .and_then(|line| line.checked_sub(1))
        })
        .ok_or_else(|| failure("not_found", format!("position {position:?} was not found")))?;
    Ok((base as i64 + offset).max(0) as usize)
}

fn parse_range(value: &str) -> Result<(i64, i64), Error> {
    let (start, end) = value.split_once("..").ok_or_else(invalid)?;
    let start = if start.is_empty() {
        i64::MIN / 2
    } else {
        start.parse().map_err(|_| invalid())?
    };
    let end = if end.is_empty() {
        i64::MAX / 2
    } else {
        end.parse().map_err(|_| invalid())?
    };
    if start > end {
        return Err(invalid());
    }
    Ok((start, end))
}

fn move_resource(request: Value) -> Result<Value, Error> {
    let source = string(&request, "source")?;
    match request.get("destination").and_then(Value::as_str) {
        Some(destination) => artist::verbs::resource_api::move_resource(&source, destination)
            .map_err(map_resource)?,
        None => artist::verbs::resource_api::delete(&source).map_err(map_resource)?,
    }
    Ok(serde_json::json!({
        "source": canonical(&source),
        "destination": request.get("destination"),
        "removed": request.get("destination").is_none(),
    }))
}

fn find(request: Value) -> Result<Value, Error> {
    let root = string(&request, "uri")?;
    let query = string(&request, "query")?;
    if query.trim().is_empty() {
        return Err(invalid());
    }
    let (mode, query) = find_query(&request, &query)?;
    let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
    if limit == 0 || limit > 500 {
        return Err(invalid());
    }
    let mut queue = vec![root];
    let mut results = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut truncated = false;
    while let Some(uri) = queue.pop() {
        if !visited.insert(uri.clone()) || visited.len() > 10_000 {
            truncated = true;
            break;
        }
        for entry in artist::verbs::resource_api::readdir(&uri).map_err(map_resource)? {
            let child = format!("{}/{}", uri.trim_end_matches('/'), entry.name);
            if find_matches(&mode, &entry.name, &child, &query) {
                if results.len() >= limit {
                    truncated = true;
                    break;
                }
                results.push(FindMatch {
                    uri: canonical(&child),
                    kind: if matches!(entry.kind, artist::verbs::resource_api::Kind::Directory) {
                        "directory"
                    } else {
                        "file"
                    },
                });
            }
            if matches!(entry.kind, artist::verbs::resource_api::Kind::Directory) {
                queue.push(child);
            }
        }
    }
    serde_json::to_value(FindResponse { results, truncated }).map_err(|_| internal())
}

fn find_query(request: &Value, query: &str) -> Result<(String, String), Error> {
    let (prefix_mode, query) = [
        ("glob:", "glob"),
        ("lit:", "literal"),
        ("literal:", "literal"),
        ("fuzzy:", "fuzzy"),
    ]
    .into_iter()
    .find_map(|(prefix, mode)| query.strip_prefix(prefix).map(|query| (mode, query)))
    .map_or((None, query), |(mode, query)| (Some(mode), query));
    let mode = request
        .get("mode")
        .and_then(Value::as_str)
        .or(prefix_mode)
        .unwrap_or_else(|| if query.contains('*') { "glob" } else { "fuzzy" });
    if !matches!(mode, "literal" | "plain" | "glob" | "fuzzy") || query.trim().is_empty() {
        return Err(invalid());
    }
    Ok((
        if mode == "plain" {
            "literal".into()
        } else {
            mode.into()
        },
        query.to_owned(),
    ))
}

fn find_matches(mode: &str, name: &str, path: &str, query: &str) -> bool {
    match mode {
        "glob" => wildcard(name, query) || wildcard(path, query),
        "literal" => {
            let query = query.to_ascii_lowercase();
            name.to_ascii_lowercase().contains(&query) || path.to_ascii_lowercase().contains(&query)
        }
        "fuzzy" => fuzzy(name, query) || fuzzy(path, query),
        _ => false,
    }
}

fn fuzzy(value: &str, query: &str) -> bool {
    let mut value = value
        .chars()
        .map(|character| character.to_ascii_lowercase());
    for wanted in query
        .chars()
        .map(|character| character.to_ascii_lowercase())
    {
        let Some(found) = value.find(|character| *character == wanted) else {
            return false;
        };
        let _ = found;
    }
    true
}

fn grep(request: Value) -> Result<Value, Error> {
    const MAX_GREP_RESOURCE: usize = 8 * 1024 * 1024;
    let root = string(&request, "uri")?;
    let query = string(&request, "query")?;
    if query.trim().is_empty() {
        return Err(invalid());
    }
    let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
    if limit == 0 || limit > 500 {
        return Err(invalid());
    }
    let mode = request
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("literal");
    if !matches!(mode, "literal" | "plain" | "regex") {
        return Err(invalid());
    }
    let regex = if mode == "regex" {
        Some(regex::Regex::new(&query).map_err(|_| invalid())?)
    } else {
        None
    };
    let mut queue = vec![root];
    let mut matches = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut truncated = false;
    while let Some(uri) = queue.pop() {
        if !visited.insert(uri.clone()) || visited.len() > 10_000 {
            truncated = true;
            break;
        }
        for entry in artist::verbs::resource_api::readdir(&uri).map_err(map_resource)? {
            let child = format!("{}/{}", uri.trim_end_matches('/'), entry.name);
            if matches!(entry.kind, artist::verbs::resource_api::Kind::Directory) {
                queue.push(child);
            } else {
                let mut bytes = read_bounded(&child, MAX_GREP_RESOURCE + 1)?;
                if bytes.len() > MAX_GREP_RESOURCE {
                    truncated = true;
                    bytes.truncate(MAX_GREP_RESOURCE);
                }
                let content = std::str::from_utf8(&bytes).map_err(|_| {
                    failure(
                        "unsupported",
                        format!("cannot grep binary resource {child:?} as UTF-8 text"),
                    )
                })?;
                for (line, text) in content.lines().enumerate() {
                    if regex.as_ref().is_some_and(|regex| regex.is_match(text))
                        || (regex.is_none() && text.contains(&query))
                    {
                        if matches.len() >= limit {
                            truncated = true;
                            break;
                        }
                        matches.push(GrepMatch {
                            uri: format!("{}#{}", canonical(&child), teca_anchor(content, line)),
                            anchor: Some(teca_anchor(content, line)),
                            content: text.to_owned(),
                        });
                    }
                }
            }
        }
    }
    serde_json::to_value(GrepResponse { matches, truncated }).map_err(|_| internal())
}

fn read_all(uri: &str) -> Result<Vec<u8>, Error> {
    const MAX_EDIT_RESOURCE: usize = 64 * 1024 * 1024;
    read_all_bounded(uri, MAX_EDIT_RESOURCE)
}

fn read_all_bounded(uri: &str, limit: usize) -> Result<Vec<u8>, Error> {
    const CHUNK_SIZE: u32 = 64 * 1024;
    let mut offset = 0u64;
    let mut bytes = Vec::new();
    loop {
        let chunk =
            artist::verbs::resource_api::read(uri, offset, CHUNK_SIZE).map_err(map_resource)?;
        if chunk.is_empty() {
            break;
        }
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(failure(
                "unsupported",
                format!("resource exceeds {limit} bytes"),
            ));
        }
        offset = offset.saturating_add(chunk.len() as u64);
        bytes.extend_from_slice(&chunk);
        if chunk.len() < CHUNK_SIZE as usize {
            break;
        }
    }
    Ok(bytes)
}

fn read_bounded(uri: &str, limit: usize) -> Result<Vec<u8>, Error> {
    const CHUNK_SIZE: u32 = 64 * 1024;
    let mut offset = 0u64;
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        let size = (limit - bytes.len()).min(CHUNK_SIZE as usize) as u32;
        let chunk = artist::verbs::resource_api::read(uri, offset, size).map_err(map_resource)?;
        if chunk.is_empty() {
            break;
        }
        offset = offset.saturating_add(chunk.len() as u64);
        bytes.extend_from_slice(&chunk);
        if chunk.len() < size as usize {
            break;
        }
    }
    Ok(bytes)
}

fn teca_lines(source: &str) -> Vec<TecaLine> {
    TecaSnapshot::from_source(source).lines().to_vec()
}

fn teca_anchor(source: &str, index: usize) -> String {
    TecaSnapshot::from_source(source)
        .lines()
        .get(index)
        .map(|line| line.anchor.clone())
        .unwrap_or_else(|| format!("teca:v1:missing:{index:x}:0:0"))
}

fn wildcard(value: &str, query: &str) -> bool {
    if query == "*" || query == "**" {
        return true;
    }
    if let Some(suffix) = query.strip_prefix("**/") {
        return value.ends_with(suffix.strip_prefix('*').unwrap_or(suffix));
    }
    if !query.contains('*') {
        return value == query;
    }
    let parts: Vec<_> = query.split('*').filter(|part| !part.is_empty()).collect();
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        let Some(found) = value[offset..].find(part) else {
            return false;
        };
        if index == 0 && !query.starts_with('*') && found != 0 {
            return false;
        }
        offset += found + part.len();
    }
    query.ends_with('*') || parts.last().is_some_and(|part| value.ends_with(part))
}

fn canonical(uri: &str) -> String {
    if uri.contains("://") {
        uri.to_owned()
    } else {
        format!("file:///{}", uri.trim_start_matches('/'))
    }
}

fn infer_mime_type(uri: &str) -> Option<String> {
    let extension = uri
        .split('?')
        .next()?
        .split('#')
        .next()?
        .rsplit('/')
        .next()?
        .rsplit_once('.')?
        .1
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "txt" | "md" | "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "java" | "c" | "h"
        | "cpp" | "toml" | "yaml" | "yml" | "json" | "xml" | "html" | "css" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => return None,
    };
    Some(mime.into())
}

fn string(value: &Value, key: &str) -> Result<String, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(invalid)
}
fn invalid() -> Error {
    failure("invalid_argument", "invalid tool request")
}
fn internal() -> Error {
    failure("internal", "default tool component failed")
}
fn failure(code: impl Into<String>, message: impl Into<String>) -> Error {
    Error {
        code: code.into(),
        message: message.into(),
        details: None,
    }
}

fn map_resource(error: artist::verbs::resource_api::Failure) -> Error {
    Error {
        code: error.code,
        message: error.message,
        details: error.details,
    }
}

struct Component;
impl exports::artist::verbs::tool::Guest for Component {
    fn invoke(request: Vec<u8>) -> Result<Vec<u8>, Error> {
        invoke(request)
    }
}
export!(Component);
