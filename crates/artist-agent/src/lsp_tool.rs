use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use dashmap::DashMap;
use rig_core::tool::{PortableTool, ToolExecutionError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::lsp::{LspClient, default_server};

static CLIENTS: OnceLock<DashMap<String, Arc<Mutex<LspClient>>>> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct LspTool {
    root: PathBuf,
    workspace: artist_tools::Workspace,
    artist: String,
}

impl LspTool {
    pub(crate) fn new(workspace: artist_tools::Workspace, artist: String) -> Self {
        Self {
            root: workspace.root().to_path_buf(),
            workspace,
            artist,
        }
    }

    async fn client(
        &self,
        file: &Path,
        server: Option<String>,
    ) -> Result<Arc<Mutex<LspClient>>, LspError> {
        let (command, args) = server_command(file, server)?;
        let key = format!("{}::{command}::{args:?}", self.root.display());
        if let Some(client) = CLIENTS.get_or_init(DashMap::new).get(&key) {
            return Ok(client.clone());
        }
        let client = Arc::new(Mutex::new(
            LspClient::start(&command, &args, &self.root)
                .await
                .map_err(|error| LspError(error.to_string()))?,
        ));
        CLIENTS
            .get_or_init(DashMap::new)
            .insert(key, client.clone());
        Ok(client)
    }

    async fn status(&self, file: &Path, server: Option<String>) -> Result<Value, LspError> {
        let (command, args) = server_command(file, server)?;
        let prefix = format!("{}::", self.root.display());
        let clients = CLIENTS
            .get_or_init(DashMap::new)
            .iter()
            .filter(|entry| entry.key().starts_with(&prefix))
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>();
        let mut active = Vec::with_capacity(clients.len());
        for client in clients {
            active.push(client.lock().await.status());
        }
        Ok(json!({
            "root": &self.root,
            "artist": self.artist,
            "file": file,
            "selectedServer": {"command": command, "args": args},
            "activeClients": active,
        }))
    }

    fn project_file(&self, input: &str) -> Result<PathBuf, LspError> {
        let file = self.root.join(input);
        let root =
            std::fs::canonicalize(&self.root).map_err(|error| LspError(error.to_string()))?;
        let file = std::fs::canonicalize(&file)
            .map_err(|error| LspError(format!("cannot open {input}: {error}")))?;
        if !file.starts_with(&root) {
            return Err(LspError(
                "LSP file must remain inside the project root".into(),
            ));
        }
        Ok(file)
    }

    async fn annotate_locations(&self, value: &mut Value, default_uri: &str) {
        let mut locations = BTreeMap::<String, BTreeSet<u64>>::new();
        collect_location_lines(value, default_uri, &mut locations);
        let mut anchors = BTreeMap::<String, BTreeMap<u64, String>>::new();
        for (uri, _lines) in locations {
            let Some(path) = uri.strip_prefix("file://") else {
                continue;
            };
            let path = PathBuf::from(path);
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            let Ok(lines_with_anchors) = self
                .workspace
                .anchors_for(&relative.to_string_lossy())
                .await
            else {
                continue;
            };
            let index = lines_with_anchors
                .into_iter()
                .map(|(line, anchor)| ((line.saturating_sub(1)) as u64, anchor))
                .collect();
            anchors.insert(uri, index);
        }
        add_location_anchors(value, default_uri, &anchors);
    }
}

fn server_command(file: &Path, server: Option<String>) -> Result<(String, Vec<String>), LspError> {
    match server {
        Some(command) => Ok((command, vec![])),
        None => default_server(file)
            .map(|(command, args)| (command.to_owned(), args))
            .ok_or_else(|| LspError("no configured LSP server for this file type".into())),
    }
}

fn collect_location_lines(
    value: &Value,
    default_uri: &str,
    locations: &mut BTreeMap<String, BTreeSet<u64>>,
) {
    match value {
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_location_lines(value, default_uri, locations)),
        Value::Object(object) => {
            let uri = object
                .get("uri")
                .and_then(Value::as_str)
                .or_else(|| object.get("targetUri").and_then(Value::as_str))
                .unwrap_or(default_uri);
            let line = object
                .get("range")
                .or_else(|| object.get("targetRange"))
                .and_then(|range| range.get("start"))
                .and_then(|start| start.get("line"))
                .and_then(Value::as_u64);
            if let Some(line) = line {
                locations.entry(uri.to_owned()).or_default().insert(line);
            }
            for value in object.values() {
                collect_location_lines(value, default_uri, locations);
            }
        }
        _ => {}
    }
}

fn add_location_anchors(
    value: &mut Value,
    default_uri: &str,
    anchors: &BTreeMap<String, BTreeMap<u64, String>>,
) {
    match value {
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| add_location_anchors(value, default_uri, anchors)),
        Value::Object(object) => {
            let uri = object
                .get("uri")
                .and_then(Value::as_str)
                .or_else(|| object.get("targetUri").and_then(Value::as_str))
                .unwrap_or(default_uri);
            let location = (|| {
                let line = object
                    .get("range")
                    .or_else(|| object.get("targetRange"))?
                    .get("start")?
                    .get("line")?
                    .as_u64()?;
                let anchor = anchors.get(uri)?.get(&line)?.clone();
                Some(json!({"path":uri,"anchor":anchor,"line":line + 1}))
            })();
            if let Some(location) = location {
                object.insert("artistLocation".into(), location);
            }
            for value in object.values_mut() {
                add_location_anchors(value, default_uri, anchors);
            }
        }
        _ => {}
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct LspArgs {
    action: String,
    file: String,
    #[serde(default)]
    line: u64,
    #[serde(default)]
    column: u64,
    server: Option<String>,
    method: Option<String>,
    #[serde(default)]
    new_name: Option<String>,
    #[serde(default)]
    include_declaration: Option<bool>,
    #[serde(default)]
    diagnostics: Vec<Value>,
    #[serde(default)]
    range: Option<Value>,
}

#[derive(thiserror::Error, Debug)]
#[error("{0}")]
pub(crate) struct LspError(String);
impl From<LspError> for ToolExecutionError {
    fn from(error: LspError) -> Self {
        ToolExecutionError::other(error.to_string()).with_code("lsp_error")
    }
}

impl PortableTool for LspTool {
    const NAME: &'static str = "lsp";
    type Error = LspError;
    type Args = LspArgs;
    type Output = Value;
    fn description(&self) -> String {
        "Query lazily shared, worktree-scoped language servers. Locations remain structured LSP locations so Artist can resolve them to current durable anchors.".into()
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"action":{"type":"string","enum":["definition","references","symbols","diagnostics","hover","rename","code_actions","request","status"]},"file":{"type":"string"},"line":{"type":"integer"},"column":{"type":"integer"},"server":{"type":"string"},"method":{"type":"string"},"new_name":{"type":"string"},"include_declaration":{"type":"boolean"},"diagnostics":{"type":"array"},"range":{}},"required":["action","file"],"additionalProperties":false})
    }
    async fn call(&self, args: LspArgs) -> Result<Value, LspError> {
        let file = self.project_file(&args.file)?;
        if args.action == "status" {
            return self.status(&file, args.server).await;
        }
        let client = self.client(&file, args.server).await?;
        let method = match args.action.as_str() {
            "definition" => "textDocument/definition",
            "references" => "textDocument/references",
            "symbols" => "textDocument/documentSymbol",
            "diagnostics" => "textDocument/diagnostic",
            "hover" => "textDocument/hover",
            "rename" => {
                if args.new_name.as_deref().unwrap_or("").is_empty() {
                    return Err(LspError("new_name is required for rename".into()));
                }
                "textDocument/rename"
            }
            "code_actions" => "textDocument/codeAction",
            "request" => args
                .method
                .as_deref()
                .ok_or_else(|| LspError("method required for request".into()))?,
            _ => return Err(LspError("unknown LSP action".into())),
        };
        let extra = json!({"new_name":args.new_name,"include_declaration":args.include_declaration,"diagnostics":args.diagnostics,"range":args.range});
        let mut client = client.lock().await;
        let result = client
            .request(method, &file, args.line, args.column, extra)
            .await
            .map_err(|error| LspError(error.to_string()))?;
        let provenance = client.provenance();
        drop(client);
        let mut result = json!({"artist":self.artist,"result":result,"provenance":provenance});
        self.annotate_locations(&mut result, &format!("file://{}", file.display()))
            .await;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotates_standard_lsp_locations_with_artist_anchors() {
        let mut response = json!({
            "uri":"file:///work/src/lib.rs",
            "range":{"start":{"line":4,"character":0},"end":{"line":4,"character":3}}
        });
        let anchors = BTreeMap::from([(
            "file:///work/src/lib.rs".into(),
            BTreeMap::from([(4, "#stable-anchor".into())]),
        )]);
        add_location_anchors(&mut response, "file:///work/src/lib.rs", &anchors);
        assert_eq!(response["artistLocation"]["anchor"], "#stable-anchor");
        assert_eq!(response["artistLocation"]["line"], 5);
    }

    #[test]
    fn status_uses_the_same_server_resolution_as_requests() {
        assert_eq!(
            server_command(Path::new("src/lib.rs"), None).unwrap(),
            ("rust-analyzer".into(), Vec::new())
        );
        assert_eq!(
            server_command(Path::new("src/lib.rs"), Some("custom-lsp".into())).unwrap(),
            ("custom-lsp".into(), Vec::new())
        );
    }

    #[test]
    fn annotates_document_symbols_using_the_requested_file_uri() {
        let mut symbol = json!({
            "name":"f",
            "range":{"start":{"line":2},"end":{"line":3}},
            "selectionRange":{"start":{"line":2},"end":{"line":2}}
        });
        let anchors = BTreeMap::from([(
            "file:///work/src/lib.rs".into(),
            BTreeMap::from([(2, "#symbol-anchor".into())]),
        )]);
        add_location_anchors(&mut symbol, "file:///work/src/lib.rs", &anchors);
        assert_eq!(symbol["artistLocation"]["anchor"], "#symbol-anchor");
    }

    #[test]
    fn annotates_location_links_with_target_locations() {
        let mut link = json!({
            "targetUri":"file:///work/src/lib.rs",
            "targetRange":{"start":{"line":9},"end":{"line":10}}
        });
        let anchors = BTreeMap::from([(
            "file:///work/src/lib.rs".into(),
            BTreeMap::from([(9, "#target-anchor".into())]),
        )]);
        add_location_anchors(&mut link, "file:///work/src/other.rs", &anchors);
        assert_eq!(link["artistLocation"]["anchor"], "#target-anchor");
    }
}
