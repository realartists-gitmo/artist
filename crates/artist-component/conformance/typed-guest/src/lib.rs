use artist::resource::types;

struct TypedTool;

fn error(message: impl ToString, uri: Option<String>) -> types::Error {
    types::Error {
        code: types::ErrorCode::Internal,
        uri,
        message: message.to_string(),
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
        requests
            .into_iter()
            .map(|request| {
                let uri = request.uri.clone();
                Ok(exports::artist::tool::read::ReadResponse::Text(text(
                    uri,
                    "artist-ast\n",
                )))
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::read::ReadResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

#[cfg(feature = "write")]
impl exports::artist::tool::write::Guest for TypedTool {
    fn write(
        requests: Vec<exports::artist::tool::write::WriteRequest>,
    ) -> Vec<Result<exports::artist::tool::write::WriteResponse, types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                Ok(exports::artist::tool::write::WriteResponse {
                    uri: request.uri,
                    text: None,
                })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::write::WriteResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

#[cfg(feature = "edit")]
impl exports::artist::tool::edit::Guest for TypedTool {
    fn edit(
        requests: Vec<exports::artist::tool::edit::EditRequest>,
    ) -> Vec<Result<exports::artist::tool::edit::EditResponse, types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                Err(error(
                    "typed conformance guest has no resource host",
                    Some(request.uri),
                ))
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::edit::EditResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

#[cfg(feature = "insert")]
impl exports::artist::tool::insert::Guest for TypedTool {
    fn insert(
        requests: Vec<exports::artist::tool::insert::InsertRequest>,
    ) -> Vec<Result<exports::artist::tool::insert::InsertResponse, types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                Err(error(
                    "typed conformance guest has no resource host",
                    Some(request.uri),
                ))
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::insert::InsertResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

#[cfg(feature = "find")]
impl exports::artist::tool::find::Guest for TypedTool {
    fn find(
        requests: Vec<exports::artist::tool::find::FindRequest>,
    ) -> Vec<Result<exports::artist::tool::find::FindResponse, types::Error>> {
        requests
            .into_iter()
            .map(|_| Ok(exports::artist::tool::find::FindResponse { uris: Vec::new() }))
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::find::FindResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

#[cfg(feature = "grep")]
impl exports::artist::tool::grep::Guest for TypedTool {
    fn grep(
        requests: Vec<exports::artist::tool::grep::GrepRequest>,
    ) -> Vec<Result<exports::artist::tool::grep::GrepResponse, types::Error>> {
        requests
            .into_iter()
            .map(|_| {
                Ok(exports::artist::tool::grep::GrepResponse {
                    matches: Vec::new(),
                })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::grep::GrepResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

#[cfg(feature = "run")]
impl exports::artist::tool::run::Guest for TypedTool {
    fn run(
        requests: Vec<exports::artist::tool::run::RunRequest>,
    ) -> Vec<Result<exports::artist::tool::run::RunResponse, types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                Ok(exports::artist::tool::run::RunResponse { uri: request.uri })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::run::RunResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

#[cfg(feature = "poll")]
impl exports::artist::tool::poll::Guest for TypedTool {
    fn poll(
        requests: Vec<exports::artist::tool::poll::PollRequest>,
    ) -> Vec<Result<exports::artist::tool::poll::PollResponse, types::Error>> {
        requests
            .into_iter()
            .map(|request| {
                Ok(exports::artist::tool::poll::PollResponse {
                    uri: request.uri.clone(),
                    text: text(request.uri, ""),
                    reason: exports::artist::tool::poll::PollReason::Timeout,
                })
            })
            .collect()
    }

    fn observe(response: Result<exports::artist::tool::poll::PollResponse, types::Error>) -> String {
        format!("{response:?}")
    }
}

macro_rules! uri_verb {
    ($feature:literal, $module:ident) => {
        #[cfg(feature = $feature)]
        impl exports::artist::tool::$module::Guest for TypedTool {
            fn $module(
                requests: Vec<exports::artist::tool::$module::UriRequest>,
            ) -> Vec<Result<exports::artist::tool::$module::UriResponse, types::Error>> {
                requests
                    .into_iter()
                    .map(|request| {
                        Ok(exports::artist::tool::$module::UriResponse { uri: request.uri })
                    })
                    .collect()
            }

            fn observe(response: Result<exports::artist::tool::$module::UriResponse, types::Error>) -> String {
                format!("{response:?}")
            }
        }
    };
}

uri_verb!("abort", abort);
uri_verb!("delete", delete);

export!(TypedTool);
