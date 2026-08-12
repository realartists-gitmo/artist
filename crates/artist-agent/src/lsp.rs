//! Lazy, project-scoped LSP clients.

use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

use crate::framed_rpc::{FramedRpc, RpcError};

pub(crate) struct LspClient {
    rpc: FramedRpc,
    /// URI -> (document version, exact last text). A shared server must see
    /// the same current project state as Artist rather than its first open.
    opened: BTreeMap<String, (u64, String)>,
    server: String,
    initialize: Value,
}

impl LspClient {
    pub(crate) async fn start(
        command: &str,
        args: &[String],
        root: &Path,
    ) -> Result<Self, RpcError> {
        let mut rpc = FramedRpc::spawn(command, args, root).await?;
        let root_uri = file_uri(root);
        let initialize = rpc
            .request_lsp(
                "initialize",
                json!({
                    "processId": null,
                    "rootUri": root_uri,
                    "capabilities": {
                        "workspace": { "workspaceEdit": { "documentChanges": true } },
                        "textDocument": {
                            "definition": {}, "references": {}, "documentSymbol": {},
                            "hover": {}, "rename": {}, "codeAction": {},
                            "publishDiagnostics": { "relatedInformation": true }
                        }
                    }
                }),
            )
            .await?;
        rpc.notify_lsp("initialized", json!({})).await?;
        Ok(Self {
            rpc,
            opened: BTreeMap::new(),
            server: command.to_owned(),
            initialize,
        })
    }

    pub(crate) async fn open(&mut self, file: &Path) -> Result<(), RpcError> {
        let uri = file_uri(file);
        let text = tokio::fs::read_to_string(file)
            .await
            .map_err(|error| RpcError::Io(error.to_string()))?;
        let Some((version, previous)) = self
            .opened
            .get(&uri)
            .map(|(version, previous)| (*version, previous.clone()))
        else {
            self.rpc
                .notify_lsp(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri, "languageId": language(file),
                            "version": 1, "text": text
                        }
                    }),
                )
                .await?;
            self.opened.insert(file_uri(file), (1, text));
            return Ok(());
        };
        if previous == text {
            return Ok(());
        }
        let next_version = version.saturating_add(1);
        self.rpc
            .notify_lsp(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": next_version },
                    "contentChanges": [{ "text": text }]
                }),
            )
            .await?;
        self.opened.insert(file_uri(file), (next_version, text));
        Ok(())
    }

    pub(crate) async fn request(
        &mut self,
        method: &str,
        file: &Path,
        line: u64,
        column: u64,
        extra: Value,
    ) -> Result<Value, RpcError> {
        self.open(file).await?;
        let uri = file_uri(file);
        let position = json!({ "line": line, "character": column });
        let params = match method {
            "textDocument/references" => json!({
                "textDocument": { "uri": uri }, "position": position,
                "context": { "includeDeclaration": extra.get("include_declaration").and_then(Value::as_bool).unwrap_or(true) }
            }),
            "textDocument/rename" => json!({
                "textDocument": { "uri": uri }, "position": position,
                "newName": extra.get("new_name").and_then(Value::as_str).unwrap_or("")
            }),
            "textDocument/codeAction" => json!({
                "textDocument": { "uri": uri },
                "range": extra.get("range").cloned().unwrap_or_else(|| json!({ "start": position, "end": position })),
                "context": { "diagnostics": extra.get("diagnostics").cloned().unwrap_or_else(|| json!([])) }
            }),
            "textDocument/documentSymbol" | "textDocument/diagnostic" => {
                json!({ "textDocument": { "uri": uri } })
            }
            _ => json!({ "textDocument": { "uri": uri }, "position": position }),
        };
        self.rpc.request_lsp(method, params).await
    }

    pub(crate) fn provenance(&self) -> Value {
        json!({"server":self.server,"initialize":self.initialize})
    }

    /// Bounded project-state projection for the model-facing `lsp.status`
    /// action. It includes no document text—only the versions currently sent
    /// to this shared server.
    pub(crate) fn status(&self) -> Value {
        json!({
            "server": self.server,
            "initialize": self.initialize,
            "openedDocuments": self.opened.iter().map(|(uri, (version, _))| {
                json!({"uri": uri, "version": version})
            }).collect::<Vec<_>>(),
        })
    }
}

pub(crate) fn default_server(file: &Path) -> Option<(&'static str, Vec<String>)> {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => Some(("rust-analyzer", vec![])),
        Some("py") => Some(("pyright-langserver", vec!["--stdio".into()])),
        Some("ts" | "tsx" | "js" | "jsx") => {
            Some(("typescript-language-server", vec!["--stdio".into()]))
        }
        _ => None,
    }
}

fn file_uri(path: &Path) -> String {
    // LSP file URIs are paths, not user-provided URLs. The virtual-path parser
    // rejects query/fragment components before this point.
    format!("file://{}", path.display())
}

fn language(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts" | "tsx") => "typescript",
        Some("js" | "jsx") => "javascript",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_common_project_servers() {
        assert_eq!(
            default_server(Path::new("x.rs")).unwrap().0,
            "rust-analyzer"
        );
        assert_eq!(
            default_server(Path::new("x.py")).unwrap().0,
            "pyright-langserver"
        );
        assert!(default_server(Path::new("x.txt")).is_none());
    }
}
