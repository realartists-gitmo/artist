#![allow(unexpected_cfgs, unused_macros)]
// Keep this shared guest source in the package dependency graph: every
// conformance package includes it, and changing the canonical v1 worlds must
// invalidate each package's component artifact.

#[cfg(feature = "read")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "read-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "write")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "write-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "edit")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "edit-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "find")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "find-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "grep")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "grep-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "run")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "run-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "send")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "send-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "abort")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "abort-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "delete")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "delete-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });
#[cfg(feature = "poll")]
wit_bindgen::generate!({ path: "../../../wit/tool-surface-v1", world: "poll-world", generate_all, additional_derives: [serde::Serialize, serde::Deserialize] });

use artist::resource::{self, types};
use serde::{Serialize, de::DeserializeOwned};
#[cfg(any(feature = "read", feature = "grep", feature = "poll"))]
use wit_bindgen::StreamReader;

struct TypedTool;

fn error(message: impl ToString, uri: Option<String>) -> types::Error {
    types::Error {
        code: types::ErrorCode::Internal,
        uri,
        message: message.to_string(),
    }
}

#[cfg(any(feature = "read", feature = "grep", feature = "poll"))]
fn one_stream<T: Serialize + DeserializeOwned + wit_stream::StreamPayload + 'static>(
    value: T,
) -> Result<StreamReader<T>, types::Error> {
    let (mut writer, reader) = wit_stream::new::<T>();
    let _ = wit_bindgen::block_on(writer.write(vec![value]));
    Ok(reader)
}

#[cfg(feature = "read")]
impl exports::artist::tool::read::Guest for TypedTool {
    fn read(
        requests: Vec<types::ReadRequest>,
    ) -> Vec<Result<types::ReadResult, types::Error>> {
        resource::read::read(&requests)
    }

    #[cfg(feature = "read")]
    fn read_stream(
        request: types::ReadRequest,
    ) -> Result<StreamReader<types::ReadResult>, types::Error> {
        let value = resource::read::read(&vec![request])
            .into_iter().next().unwrap_or_else(|| Err(error("host returned no read result", None)))?;
        one_stream(value)
    }
}

#[cfg(feature = "write")]
impl exports::artist::tool::write::Guest for TypedTool {
    fn write(
        requests: Vec<types::WriteRequest>,
    ) -> Vec<Result<types::WriteResult, types::Error>> {
        resource::write::write(&requests)
    }
}

#[cfg(feature = "edit")]
impl exports::artist::tool::edit::Guest for TypedTool {
    fn edit(
        requests: Vec<types::EditRequest>,
    ) -> Vec<Result<types::EditResult, types::Error>> {
        resource::edit::edit(&requests)
    }
}

#[cfg(feature = "find")]
impl exports::artist::tool::find::Guest for TypedTool {
    fn find(request: types::FindRequest) -> Result<Vec<String>, types::Error> {
        resource::find::find(&request)
    }
}

#[cfg(feature = "grep")]
impl exports::artist::tool::grep::Guest for TypedTool {
    fn grep(request: types::GrepRequest) -> Result<Vec<types::AnchoredText>, types::Error> {
        resource::grep::grep(&request)
    }

    #[cfg(feature = "grep")]
    fn grep_stream(
        request: types::GrepRequest,
    ) -> Result<StreamReader<types::AnchoredText>, types::Error> {
        let values = Self::grep(request)?;
        let (mut writer, reader) = wit_stream::new::<types::AnchoredText>();
        let _ = wit_bindgen::block_on(writer.write(values));
        Ok(reader)
    }
}

#[cfg(feature = "run")]
impl exports::artist::tool::run::Guest for TypedTool {
    fn run(
        requests: Vec<types::RunRequest>,
    ) -> Vec<Result<String, types::Error>> {
        resource::run::run(&requests)
    }
}

#[cfg(feature = "send")]
impl exports::artist::tool::send::Guest for TypedTool {
    fn send(
        requests: Vec<types::SendRequest>,
    ) -> Vec<Result<String, types::Error>> {
        resource::send::send(&requests)
    }
}

macro_rules! uri_verb {
    ($module:ident, $import:ident, $method:ident) => {
        impl exports::artist::tool::$module::Guest for TypedTool {
            fn $module(
                uris: Vec<String>,
            ) -> Vec<Result<String, types::Error>> {
                let mut results = Vec::with_capacity(uris.len());
                for uri in uris {
                    let result = resource::$import::$method(&vec![uri.clone()])
                        .into_iter().next().unwrap_or_else(|| Err(error("host returned no result", Some(uri))));
                    results.push(result);
                }
                results
            }
        }
    };
}

#[cfg(feature = "abort")]
uri_verb!(abort, abort, abort);
#[cfg(feature = "delete")]
uri_verb!(delete, delete, delete);

#[cfg(feature = "poll")]
impl exports::artist::tool::poll::Guest for TypedTool {
    fn poll(request: types::PollRequest) -> Result<types::PollResult, types::Error> {
        resource::poll::poll(&request)
    }

    #[cfg(feature = "poll")]
    fn poll_stream(
        request: types::PollRequest,
    ) -> Result<StreamReader<types::AnchoredText>, types::Error> {
        let result = Self::poll(request)?;
        let (mut writer, reader) = wit_stream::new::<types::AnchoredText>();
        let _ = wit_bindgen::block_on(writer.write(result.text));
        Ok(reader)
    }
}

export!(TypedTool);
