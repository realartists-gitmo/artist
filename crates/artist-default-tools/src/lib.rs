//! Rust guest implementation of the default file-shaped tools.

wit_bindgen::generate!({
    path: "../artist-kernel/wasm/verbs/wit",
    world: "tool-extension",
});

use serde::Serialize;
use serde_json::Value;

#[derive(serde::Deserialize)]
struct Envelope { tool: String, request: Value }

#[derive(Serialize)]
struct ReadResponse { uri: String, position: Option<String>, range: String, lines: Vec<ReadLine> }
#[derive(Clone, Serialize)]
struct ReadLine { anchor: String, content: String }
#[derive(Serialize)]
struct FindResponse { results: Vec<FindMatch>, next_cursor: Option<String> }
#[derive(Serialize)]
struct FindMatch { uri: String, kind: &'static str }
#[derive(Serialize)]
struct GrepResponse { matches: Vec<GrepMatch>, next_cursor: Option<String> }
#[derive(Serialize)]
struct GrepMatch { uri: String, content: String }

type Error = exports::artist::verbs::tool::Error;

fn invoke(request: Vec<u8>) -> Result<Vec<u8>, Error> {
    let input: Value = toon_format::decode_default(std::str::from_utf8(&request).map_err(|_| invalid())?).map_err(|_| invalid())?;
    let envelope: Envelope = serde_json::from_value(input).map_err(|_| invalid())?;
    let response = match envelope.tool.as_str() {
        "read" => read(envelope.request)?,
        "write" => write(envelope.request)?,
        "edit" => edit(envelope.request)?,
        "move" => move_resource(envelope.request)?,
        "find" => find(envelope.request)?,
        "grep" => grep(envelope.request)?,
        _ => return Err(Error::Unsupported),
    };
    toon_format::encode_default(&response).map(|value| value.into_bytes()).map_err(|_| internal())
}

fn read(request: Value) -> Result<Value, Error> {
    let requested_uri = string(&request, "uri")?;
    let (uri, position) = split_fragment(&requested_uri);
    let bytes = artist::verbs::resource_api::read(&uri, 0, u32::MAX).map_err(map_resource)?;
    let source = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    let all: Vec<_> = source.lines().enumerate().map(|(index, content)| ReadLine { anchor: format!("line:{index}"), content: content.to_owned() }).collect();
    let position_index = position.as_deref().map(|value| resolve_position(&all, value)).transpose()?.unwrap_or(0);
    let range = request.get("range").and_then(Value::as_str).unwrap_or("-200..+200");
    let (start, end) = parse_range(range)?;
    let start = (position_index as i64 + start).max(0) as usize;
    let end = (position_index as i64 + end + 1).max(0) as usize;
    let lines = all.get(start..end.min(all.len())).unwrap_or_default().to_vec();
    Ok(serde_json::to_value(ReadResponse { uri: canonical(&uri), position, range: range.to_owned(), lines }).map_err(|_| internal())?)
}

fn write(request: Value) -> Result<Value, Error> {
    let uri = string(&request, "uri")?;
    if uri.contains('?') || uri.contains('#') { return Err(invalid()); }
    let content = string(&request, "content")?;
    let _ = artist::verbs::resource_api::truncate(&uri, 0);
    artist::verbs::resource_api::write(&uri, 0, content.as_bytes()).map_err(map_resource)?;
    Ok(Value::Object(serde_json::Map::new()))
}

fn edit(request: Value) -> Result<Value, Error> {
    let uri = string(&request, "uri")?;
    if uri.contains('?') || uri.contains('#') { return Err(invalid()); }
    let mut source = String::from_utf8(
        artist::verbs::resource_api::read(&uri, 0, u32::MAX).map_err(map_resource)?,
    ).map_err(|_| invalid())?;
    let changes = request.get("changes").and_then(Value::as_array).ok_or_else(invalid)?;
    for change in changes {
        let anchor = string(change, "anchor")?;
        let line = anchor.strip_prefix("line:").and_then(|value| value.parse::<usize>().ok()).ok_or_else(invalid)?;
        let replacement = change.get("content").or_else(|| change.get("replacement")).and_then(Value::as_str).ok_or_else(invalid)?;
        let mut lines: Vec<String> = source.lines().map(ToOwned::to_owned).collect();
        if line >= lines.len() { return Err(Error::NotFound); }
        lines[line] = replacement.to_owned();
        source = lines.join("\n");
        if source.ends_with('\n') { source.push('\n'); }
    }
    let _ = artist::verbs::resource_api::truncate(&uri, 0);
    artist::verbs::resource_api::write(&uri, 0, source.as_bytes()).map_err(map_resource)?;
    Ok(Value::Object(serde_json::Map::new()))
}

fn split_fragment(uri: &str) -> (String, Option<String>) {
    uri.split_once('#').map(|(base, fragment)| (base.to_owned(), Some(fragment.to_owned()))).unwrap_or_else(|| (uri.to_owned(), None))
}

fn resolve_position(lines: &[ReadLine], position: &str) -> Result<usize, Error> {
    let (anchor, offset) = if let Some((anchor, suffix)) = position.rsplit_once('+') {
        (anchor, suffix.parse::<i64>().map_err(|_| invalid())?)
    } else if let Some((anchor, suffix)) = position.rsplit_once('-').filter(|(_, suffix)| !suffix.is_empty()) {
        (anchor, -suffix.parse::<i64>().map_err(|_| invalid())?)
    } else { (position, 0) };
    let base = lines.iter().position(|line| line.anchor == anchor).or_else(|| anchor.parse::<usize>().ok().and_then(|line| line.checked_sub(1))).ok_or(Error::NotFound)?;
    Ok((base as i64 + offset).max(0) as usize)
}

fn parse_range(value: &str) -> Result<(i64, i64), Error> {
    let (start, end) = value.split_once("..").ok_or_else(invalid)?;
    let start = if start.is_empty() { i64::MIN / 2 } else { start.parse().map_err(|_| invalid())? };
    let end = if end.is_empty() { i64::MAX / 2 } else { end.parse().map_err(|_| invalid())? };
    if start > end { return Err(invalid()); }
    Ok((start, end))
}

fn move_resource(request: Value) -> Result<Value, Error> {
    let source = string(&request, "source")?;
    match request.get("destination").and_then(Value::as_str) {
        Some(destination) => artist::verbs::resource_api::move_resource(&source, destination).map_err(map_resource)?,
        None => artist::verbs::resource_api::delete(&source).map_err(map_resource)?,
    }
    Ok(Value::Object(serde_json::Map::new()))
}

fn find(request: Value) -> Result<Value, Error> {
    let root = string(&request, "uri")?;
    let query = string(&request, "query")?;
    let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let offset = request.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut queue = vec![root];
    let mut results = Vec::new();
    while let Some(uri) = queue.pop() {
        for entry in artist::verbs::resource_api::readdir(&uri).map_err(map_resource)? {
            let child = format!("{}/{}", uri.trim_end_matches('/'), entry.name);
            if wildcard(&entry.name, &query) || wildcard(&child, &query) {
                results.push(FindMatch { uri: canonical(&child), kind: if matches!(entry.kind, artist::verbs::resource_api::Kind::Directory) { "directory" } else { "file" } });
            }
            if matches!(entry.kind, artist::verbs::resource_api::Kind::Directory) { queue.push(child); }
        }
    }
    let more = results.len() >= offset.saturating_add(limit) && limit > 0;
    let results = results.into_iter().skip(offset).take(limit).collect();
    Ok(serde_json::to_value(FindResponse { results, next_cursor: more.then(|| offset.saturating_add(limit).to_string()) }).map_err(|_| internal())?)
}

fn grep(request: Value) -> Result<Value, Error> {
    let root = string(&request, "uri")?;
    let query = string(&request, "query")?;
    let limit = request.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let mut queue = vec![root];
    let mut matches = Vec::new();
    while let Some(uri) = queue.pop() {
        for entry in artist::verbs::resource_api::readdir(&uri).map_err(map_resource)? {
            let child = format!("{}/{}", uri.trim_end_matches('/'), entry.name);
            if matches!(entry.kind, artist::verbs::resource_api::Kind::Directory) {
                queue.push(child);
            } else if let Ok(bytes) = artist::verbs::resource_api::read(&child, 0, u32::MAX) {
                if let Ok(content) = std::str::from_utf8(&bytes) {
                    for (line, text) in content.lines().enumerate() {
                        if text.contains(&query) {
                            matches.push(GrepMatch { uri: format!("{}#{}", canonical(&child), line + 1), content: text.to_owned() });
                        }
                    }
                }
            }
        }
    }
    matches.truncate(limit);
    Ok(serde_json::to_value(GrepResponse { matches, next_cursor: None }).map_err(|_| internal())?)
}

fn wildcard(value: &str, query: &str) -> bool {
    if query == "*" || query == "**" { return true; }
    if let Some(suffix) = query.strip_prefix("**/") {
        return value.ends_with(suffix.strip_prefix('*').unwrap_or(suffix));
    }
    if !query.contains('*') { return value == query; }
    let parts: Vec<_> = query.split('*').filter(|part| !part.is_empty()).collect();
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        let Some(found) = value[offset..].find(part) else { return false; };
        if index == 0 && !query.starts_with('*') && found != 0 { return false; }
        offset += found + part.len();
    }
    query.ends_with('*') || parts.last().is_some_and(|part| value.ends_with(part))
}

fn canonical(uri: &str) -> String {
    if uri.contains("://") { uri.to_owned() } else { format!("file:///{}", uri.trim_start_matches('/')) }
}

fn string(value: &Value, key: &str) -> Result<String, Error> { value.get(key).and_then(Value::as_str).map(ToOwned::to_owned).ok_or_else(invalid) }
fn invalid() -> Error { Error::InvalidArgument }
fn internal() -> Error { Error::Internal }

fn map_resource(error: artist::verbs::resource_api::Error) -> Error {
    match error {
        artist::verbs::resource_api::Error::InvalidArgument => invalid(),
        artist::verbs::resource_api::Error::NotFound => Error::NotFound,
        artist::verbs::resource_api::Error::PermissionDenied => Error::PermissionDenied,
        artist::verbs::resource_api::Error::Unsupported => Error::Unsupported,
        artist::verbs::resource_api::Error::Conflict => Error::Conflict,
        artist::verbs::resource_api::Error::Io => internal(),
    }
}

struct Component;
impl exports::artist::verbs::tool::Guest for Component {
    fn invoke(request: Vec<u8>) -> Result<Vec<u8>, Error> { invoke(request) }
}
export!(Component);
