wit_bindgen::generate!({
    path: "../../../wit",
});

use artist::component::types;

struct VerbTool;

fn verb() -> &'static str {
    match env!("CARGO_PKG_NAME").rsplit('-').next().unwrap_or_default() {
        "read" | "write" | "edit" | "poll" | "send" | "run" | "abort" | "delete"
        | "find" | "grep" => env!("CARGO_PKG_NAME").rsplit('-').next().unwrap(),
        _ => "unknown",
    }
}

impl exports::artist::component::component::Guest for VerbTool {
    fn info() -> types::ComponentInfo {
        let verb = verb();
        types::ComponentInfo {
            name: env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            abi_version: "0.1".to_owned(),
            interfaces: vec![format!("artist:tool/{verb}")],
            required_capabilities: vec![format!("resource.{verb}")],
        }
    }

    fn invoke(request: types::Invocation) -> Result<types::Response, types::Error> {
        let verb = verb();
        if verb == "unknown" {
            return Err(types::Error {
                kind: types::ErrorKind::InvalidRequest,
                message: "tool package name must end in a universal verb".to_owned(),
                details: None,
            });
        }
        let response = artist::component::host::invoke(&types::HostRequest {
            operation: format!("tool.{verb}"),
            target: request.target,
            input: request.input,
            capability: Some(format!("resource.{verb}")),
            handle: None,
        })?;
        Ok(types::Response {
            output: response.output,
            metadata: response.metadata,
        })
    }

    fn invoke_stream(request: types::Invocation) -> Result<Vec<types::StreamChunk>, types::Error> {
        let response = Self::invoke(request)?;
        Ok(vec![types::StreamChunk {
            sequence: 0,
            final_: true,
            output: response.output,
            metadata: response.metadata,
        }])
    }
}

export!(VerbTool);
