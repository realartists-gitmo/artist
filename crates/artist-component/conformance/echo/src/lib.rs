wit_bindgen::generate!({
    path: "../../wit",
});

use artist::component::types;

struct Echo;

impl exports::artist::component::component::Guest for Echo {
    fn info() -> types::ComponentInfo {
        types::ComponentInfo {
            name: "conformance-echo".to_owned(),
            version: "0.1.0".to_owned(),
            abi_version: "0.1".to_owned(),
            interfaces: vec!["artist:component/component".to_owned()],
            required_capabilities: Vec::new(),
        }
    }

    fn invoke(
        request: types::Invocation,
    ) -> Result<types::Response, types::Error>
    {
        let operation = request.operation.clone();
        if let Some(verb) = operation.strip_prefix("tool.").map(str::to_owned) {
            let host = artist::component::host::invoke(&types::HostRequest {
                operation,
                target: request.target,
                input: request.input,
                capability: Some(format!("resource.{verb}")),
                handle: None,
            })?;
            return Ok(types::Response {
                output: host.output,
                metadata: host.metadata,
            });
        }
        if request.operation == "error" {
            return Err(types::Error {
                kind: types::ErrorKind::InvalidRequest,
                message: "requested conformance error".to_owned(),
                details: Some(request.input),
            });
        }
        if request.operation == "malformed" {
            return Ok(types::Response {
                output: "not-json".to_owned(),
                metadata: "{}".to_owned(),
            });
        }
        if request.operation == "host" {
            let host = artist::component::host::invoke(&types::HostRequest {
                operation: "probe".to_owned(),
                target: request.target,
                input: request.input,
                capability: Some("resource.read".to_owned()),
                handle: None,
            })?;
            return Ok(types::Response {
                output: host.output,
                metadata: host.metadata,
            });
        }
        if request.operation == "handle" {
            let handle = artist::component::host::acquire(&request.target)?;
            let borrowed = artist::component::host::borrow_handle(&handle)?;
            let response = artist::component::host::invoke(&types::HostRequest {
                operation: "handle-probe".to_owned(),
                target: request.target,
                input: request.input,
                capability: None,
                handle: Some(borrowed),
            })?;
            artist::component::host::release(&handle)?;
            return Ok(types::Response {
                output: response.output,
                metadata: response.metadata,
            });
        }
        Ok(types::Response {
            output: request.input,
            metadata: "{\"component\":\"conformance-echo\"}".to_owned(),
        })
    }

    fn invoke_stream(
        request: types::Invocation,
    ) -> Result<Vec<types::StreamChunk>, types::Error> {
        Ok(vec![
            types::StreamChunk {
                sequence: 0,
                final_: false,
                output: request.input,
                metadata: "{\"chunk\":0}".to_owned(),
            },
            types::StreamChunk {
                sequence: 1,
                final_: true,
                output: "{\"done\":true}".to_owned(),
                metadata: "{\"chunk\":1}".to_owned(),
            },
        ])
    }
}

export!(Echo);
