//! The verb contract host side: the batch-native family shape and dispatch.
//!
//! A tool is a component exporting one or more verb interfaces. Every verb
//! exports a single function taking `list<request>` and returning
//! `list<result<response, error>>`. The model surface stays scalar; the
//! harness batches scalar tool calls into one component call.
//!
//! Specific verbs are not defined yet, so [`VerbDispatcher`] is parameterized
//! by the request/response types: each concrete verb family brings its own
//! typed records and a [`VerbTool`] implementation (built on its bindings),
//! then registers with a dispatcher.

use std::collections::HashMap;

use async_trait::async_trait;

/// The verb error algebra, mirroring `artist:verbs/types.error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerbError {
    InvalidArgument,
    NotFound,
    Unsupported,
    PermissionDenied,
    Conflict,
    Aborted,
    Internal,
}

/// A registered tool implementing one verb family.
///
/// `Req`/`Res` are the verb's own flat, scalar request/response records from
/// its exported WIT interface. `call` is batch-native: one wasm call, many
/// requests in, one result per request out.
#[async_trait]
pub trait VerbTool<Req, Res>: Send + Sync {
    fn name(&self) -> &str;

    async fn call(&self, requests: Vec<Req>) -> Vec<Result<Res, VerbError>>;
}

/// Routes batches to the registered tool implementing a verb family.
#[derive(Default)]
pub struct VerbDispatcher<Req, Res> {
    tools: HashMap<String, Box<dyn VerbTool<Req, Res>>>,
}

impl<Req, Res> VerbDispatcher<Req, Res> {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T: VerbTool<Req, Res> + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.name().to_string(), Box::new(tool));
    }

    pub fn get(&self, name: &str) -> Option<&dyn VerbTool<Req, Res>> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(|s| s.as_str())
    }

    /// Dispatch a batch to the tool named `name`.
    pub async fn dispatch(
        &self,
        name: &str,
        requests: Vec<Req>,
    ) -> Option<Vec<Result<Res, VerbError>>> {
        match self.get(name) {
            Some(tool) => Some(tool.call(requests).await),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct PingReq {
        value: u64,
    }

    #[derive(Debug, PartialEq)]
    struct PingRes {
        echo: u64,
    }

    struct PingTool;

    #[async_trait]
    impl VerbTool<PingReq, PingRes> for PingTool {
        fn name(&self) -> &str {
            "ping"
        }

        async fn call(&self, requests: Vec<PingReq>) -> Vec<Result<PingRes, VerbError>> {
            requests
                .into_iter()
                .map(|req| Ok(PingRes { echo: req.value }))
                .collect()
        }
    }

    #[tokio::test]
    async fn batch_native_dispatch() {
        let mut dispatcher = VerbDispatcher::new();
        dispatcher.register(PingTool);

        let results = dispatcher
            .dispatch("ping", vec![PingReq { value: 1 }, PingReq { value: 2 }])
            .await
            .unwrap();
        assert_eq!(
            results,
            vec![Ok(PingRes { echo: 1 }), Ok(PingRes { echo: 2 })]
        );

        assert_eq!(dispatcher.dispatch("pong", vec![]).await, None);
    }
}
