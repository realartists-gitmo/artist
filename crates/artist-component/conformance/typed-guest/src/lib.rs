use artist::resource::types;
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
        requests
            .into_iter()
            .map(|request| {
                let uri = request.uri.clone();
                let content = if uri == "resources://ast/resource.md" {
                    "artist-ast\n".to_owned()
                } else {
                    let path = uri
                        .strip_prefix("file://")
                        .unwrap_or(&uri)
                        .replace("%20", " ");
                    std::fs::read_to_string(path).unwrap_or_else(|_| "artist-ast\n".to_owned())
                };
                let lines = content
                    .lines()
                    .enumerate()
                    .map(|(index, text)| types::AnchoredLine {
                        anchor: format!("line-{}", index + 1),
                        text: text.to_owned(),
                        ending: types::LineEnding::Lf,
                    })
                    .collect();
                Ok(types::ReadResult::Text(types::AnchoredText { uri, lines }))
            })
            .collect()
    }

    #[cfg(feature = "read")]
    fn read_stream(
        request: types::ReadRequest,
    ) -> Result<StreamReader<types::ReadResult>, types::Error> {
        Self::read(vec![request])
            .into_iter()
            .next()
            .unwrap_or_else(|| Err(error("tool guest returned no read result", None)))
            .and_then(|value| one_stream(value))
    }
}

#[cfg(feature = "write")]
impl exports::artist::tool::write::Guest for TypedTool {
    fn write(
        requests: Vec<types::WriteRequest>,
    ) -> Vec<Result<types::WriteResult, types::Error>> {
        requests.into_iter().map(|request| Err(error("tool guest has no resource host", Some(request.uri)))).collect()
    }
}

#[cfg(feature = "edit")]
impl exports::artist::tool::edit::Guest for TypedTool {
    fn edit(
        requests: Vec<types::EditRequest>,
    ) -> Vec<Result<types::EditResult, types::Error>> {
        requests.into_iter().map(|request| Err(error("tool guest has no resource host", Some(request.uri)))).collect()
    }
}

#[cfg(feature = "find")]
impl exports::artist::tool::find::Guest for TypedTool {
    fn find(request: types::FindRequest) -> Result<Vec<String>, types::Error> {
        let _ = request;
        Err(error("tool guest has no resource host", None))
    }
}

#[cfg(feature = "grep")]
impl exports::artist::tool::grep::Guest for TypedTool {
    fn grep(request: types::GrepRequest) -> Result<Vec<types::AnchoredText>, types::Error> {
        let _ = request;
        Err(error("tool guest has no resource host", None))
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
        requests.into_iter().map(|request| Err(error("tool guest has no resource host", Some(request.uri)))).collect()
    }
}

#[cfg(feature = "send")]
impl exports::artist::tool::send::Guest for TypedTool {
    fn send(
        requests: Vec<types::SendRequest>,
    ) -> Vec<Result<String, types::Error>> {
        requests.into_iter().map(|request| Err(error("tool guest has no resource host", Some(request.uri)))).collect()
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
                    let result = Err(error("tool guest has no resource host", Some(uri)));
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
        let _ = request;
        Err(error("tool guest has no resource host", None))
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
