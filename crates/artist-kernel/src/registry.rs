use crate::{
    BatchRequest, BatchResult, ClaimDecision, Handler, HandlerDescriptor, InvocationContext,
    InvocationScope, ItemResult, KernelError, KernelHandle, Operation, OperationResult, Request,
    ResourceCatalogEntry, ResourceCatalogProvider, ResourceUri, ToolDefinition, ToolProvider,
    TypedHandler, Verb,
};
use std::any::Any;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::RwLock;

struct Inner {
    handlers: RwLock<Vec<Arc<dyn Handler>>>,
    typed_handlers: RwLock<Vec<Arc<dyn TypedHandler>>>,
    tool_providers: RwLock<Vec<Arc<dyn ToolProvider>>>,
    resource_catalog_providers: RwLock<Vec<Arc<dyn ResourceCatalogProvider>>>,
    background: Mutex<Vec<Box<dyn Any + Send>>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Anchor, AnchoredLine, AnchoredText, BoxFuture, EnvironmentEntry, LineEnding, PollResult,
        RegexAtom,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeState {
        resources: HashMap<String, (String, bool)>,
        contexts: HashMap<String, InvocationContext>,
        runs: u64,
    }

    #[derive(Clone)]
    struct FakeNamespace {
        state: Arc<Mutex<FakeState>>,
        create_on_send: bool,
        distinct_run: bool,
    }

    impl FakeNamespace {
        fn new(create_on_send: bool, distinct_run: bool) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeState::default())),
                create_on_send,
                distinct_run,
            }
        }

        fn uri(&self, value: &str) -> ResourceUri {
            ResourceUri::parse(value).unwrap()
        }

        fn input(&self, value: &str) -> String {
            self.state
                .lock()
                .unwrap()
                .resources
                .get(value)
                .map(|(input, _)| input.clone())
                .unwrap_or_default()
        }
    }

    impl TypedHandler for FakeNamespace {
        fn descriptor(&self) -> HandlerDescriptor {
            HandlerDescriptor {
                name: "fake-namespace".to_owned(),
                schemes: vec!["fake".to_owned()],
                verbs: vec![Verb::Run, Verb::Send],
            }
        }

        fn claims_operation(&self, operation: &Operation) -> bool {
            let uris = match operation {
                Operation::Run(requests) => requests
                    .iter()
                    .map(|request| &request.uri)
                    .collect::<Vec<_>>(),
                Operation::Send(requests) => requests
                    .iter()
                    .map(|request| &request.uri)
                    .collect::<Vec<_>>(),
                _ => return false,
            };
            !uris.is_empty() && uris.iter().all(|uri| uri.scheme() == "fake")
        }

        fn execute_typed<'a>(
            &'a self,
            operation: Operation,
            _host: KernelHandle,
            context: InvocationContext,
        ) -> BoxFuture<'a, Result<OperationResult, KernelError>> {
            let state = Arc::clone(&self.state);
            let create_on_send = self.create_on_send;
            let distinct_run = self.distinct_run;
            Box::pin(async move {
                let mut state = state.lock().unwrap();
                match operation {
                    Operation::Run(mut requests) => {
                        let request = requests.pop().unwrap();
                        let requested = request.uri.to_string();
                        if state.resources.contains_key(&requested) {
                            return Err(KernelError::Conflict { uri: requested });
                        }
                        state.runs += 1;
                        let result = if distinct_run {
                            format!("fake://execution-{}", state.runs)
                        } else {
                            requested
                        };
                        state
                            .resources
                            .insert(result.clone(), (String::new(), true));
                        state.contexts.insert(result.clone(), context);
                        Ok(OperationResult::Run(vec![Ok(
                            ResourceUri::parse(&result).unwrap()
                        )]))
                    }
                    Operation::Send(requests) => {
                        let mut results = Vec::new();
                        for request in requests {
                            let uri = request.uri.to_string();
                            if !state.resources.contains_key(&uri) {
                                if !create_on_send {
                                    results.push(Err(KernelError::NotFound { uri }));
                                    continue;
                                }
                                state.resources.insert(uri.clone(), (String::new(), true));
                                state.contexts.insert(uri.clone(), context.clone());
                            }
                            let (input, receptive) = state.resources.get_mut(&uri).unwrap();
                            if !*receptive {
                                results.push(Err(KernelError::Conflict { uri }));
                            } else {
                                input.push_str(&request.content);
                                results.push(Ok(request.uri));
                            }
                        }
                        Ok(OperationResult::Send(results))
                    }
                    _ => unreachable!(),
                }
            })
        }
    }

    #[test]
    fn poll_regex_matches_across_anchored_lines() {
        let uri = ResourceUri::parse("file:///tmp/poll.txt").unwrap();
        let condition = crate::PollCondition::Atom(crate::PollAtom::Regex(RegexAtom {
            target: 0,
            pattern: "hello\\nworld".to_owned(),
        }));
        let result = PollResult {
            text: vec![AnchoredText {
                uri,
                lines: vec![
                    AnchoredLine {
                        anchor: Anchor::from_tokens(vec!["1".to_owned()]),
                        text: "hello".to_owned(),
                        ending: LineEnding::Lf,
                    },
                    AnchoredLine {
                        anchor: Anchor::from_tokens(vec!["2".to_owned()]),
                        text: "world".to_owned(),
                        ending: LineEnding::Lf,
                    },
                ],
            }],
            satisfied: Vec::new(),
        };
        assert!(
            evaluate_poll_condition(Some(&condition), &[result], tokio::time::Duration::ZERO,).0
        );
    }

    #[tokio::test]
    async fn run_and_send_are_owned_by_uri_handlers() {
        let kernel = Kernel::new();
        let namespace = FakeNamespace::new(true, false);
        kernel.register_typed(namespace.clone()).await;
        let context = InvocationContext {
            working_uri: Some(namespace.uri("fake://workspace")),
            environment: vec![EnvironmentEntry {
                name: "MODE".to_owned(),
                value: "test".to_owned(),
            }],
            ..InvocationContext::default()
        };
        let uri = namespace.uri("fake://run-tests");
        let run = kernel
            .execute_operation_with_context(
                Operation::Run(vec![crate::RunRequest {
                    uri: uri.clone(),
                    args: vec!["--release".to_owned()],
                }]),
                context.clone(),
            )
            .await
            .unwrap();
        assert!(matches!(run, OperationResult::Run(ref values) if values[0].as_ref() == Ok(&uri)));

        let rerun = kernel
            .execute_operation(Operation::Run(vec![crate::RunRequest {
                uri: uri.clone(),
                args: Vec::new(),
            }]))
            .await
            .unwrap();
        assert!(
            matches!(rerun, OperationResult::Run(ref values) if matches!(values[0], Err(KernelError::Conflict { .. })))
        );

        let send = kernel
            .execute_operation_with_context(
                Operation::Send(vec![
                    crate::SendRequest {
                        uri: uri.clone(),
                        content: "abc".to_owned(),
                    },
                    crate::SendRequest {
                        uri: uri.clone(),
                        content: "def\n".to_owned(),
                    },
                ]),
                context,
            )
            .await
            .unwrap();
        assert!(
            matches!(send, OperationResult::Send(ref values) if values.iter().all(Result::is_ok))
        );
        assert_eq!(namespace.input("fake://run-tests"), "abcdef\n");
        assert_eq!(namespace.state.lock().unwrap().contexts.len(), 1);

        let mut state = namespace.state.lock().unwrap();
        state.resources.get_mut("fake://run-tests").unwrap().1 = false;
        drop(state);
        let rejected = kernel
            .execute_operation(Operation::Send(vec![crate::SendRequest {
                uri,
                content: "replacement".to_owned(),
            }]))
            .await
            .unwrap();
        assert!(
            matches!(rejected, OperationResult::Send(ref values) if matches!(values[0], Err(KernelError::Conflict { .. })))
        );
        assert_eq!(namespace.input("fake://run-tests"), "abcdef\n");

        let distinct = FakeNamespace::new(true, true);
        let distinct_kernel = Kernel::new();
        distinct_kernel.register_typed(distinct.clone()).await;
        let source = distinct.uri("fake://source");
        let result = distinct_kernel
            .execute_operation(Operation::Run(vec![crate::RunRequest {
                uri: source.clone(),
                args: Vec::new(),
            }]))
            .await
            .unwrap();
        assert!(
            matches!(result, OperationResult::Run(ref values) if values[0].as_ref().is_ok_and(|value| value != &source))
        );
    }

    #[tokio::test]
    async fn send_creation_order_and_missing_policy_stay_inside_handlers() {
        let kernel = Kernel::new();
        let namespace = FakeNamespace::new(true, false);
        kernel.register_typed(namespace.clone()).await;
        let uri = namespace.uri("fake://concurrent");
        let first = kernel.clone();
        let second = kernel.clone();
        let (first, second) = tokio::join!(
            first.execute_operation(Operation::Send(vec![crate::SendRequest {
                uri: uri.clone(),
                content: "abc".to_owned()
            }])),
            second.execute_operation(Operation::Send(vec![crate::SendRequest {
                uri: uri.clone(),
                content: "def\n".to_owned()
            }])),
        );
        assert!(first.is_ok() && second.is_ok());
        assert_eq!(namespace.state.lock().unwrap().resources.len(), 1);
        assert!(matches!(
            namespace.input("fake://concurrent").as_str(),
            "abcdef\n" | "def\nabc"
        ));

        let no_create = FakeNamespace::new(false, false);
        let no_create_kernel = Kernel::new();
        no_create_kernel.register_typed(no_create.clone()).await;
        let result = no_create_kernel
            .execute_operation(Operation::Send(vec![crate::SendRequest {
                uri: no_create.uri("fake://missing"),
                content: "ignored".to_owned(),
            }]))
            .await
            .unwrap();
        assert!(
            matches!(result, OperationResult::Send(ref values) if matches!(values[0], Err(KernelError::NotFound { .. })))
        );

        let unknown = Kernel::new()
            .execute_operation(Operation::Send(vec![crate::SendRequest {
                uri: ResourceUri::parse("unknown://missing").unwrap(),
                content: "ignored".to_owned(),
            }]))
            .await;
        assert!(matches!(
            unknown,
            Ok(OperationResult::Send(ref values))
                if matches!(values[0], Err(KernelError::UnsupportedVerb { .. }))
        ));
    }

    #[tokio::test]
    async fn duplicate_write_uris_are_rejected_before_any_mutation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("duplicate.txt");
        std::fs::write(&path, "original\n").unwrap();
        let kernel = Kernel::new();
        kernel
            .register_typed(crate::FileHandler::new(root.path()).unwrap())
            .await;
        let uri = ResourceUri::parse(&path.display().to_string()).unwrap();
        let result = kernel
            .execute_operation(Operation::Write(vec![
                crate::WriteRequest {
                    uri: uri.clone(),
                    content: "first\n".to_owned(),
                },
                crate::WriteRequest {
                    uri: uri.clone(),
                    content: "second\n".to_owned(),
                },
                crate::WriteRequest {
                    uri,
                    content: "third\n".to_owned(),
                },
            ]))
            .await
            .unwrap();
        assert!(
            matches!(result, OperationResult::Write(ref values) if values.len() == 3 && values.iter().all(|value| matches!(value, Err(KernelError::InvalidRequest { .. }))))
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original\n");
    }

    #[tokio::test]
    async fn uri_fragments_lower_only_to_existing_positions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fragment.txt");
        std::fs::write(&path, "first\nsecond\n").unwrap();
        let kernel = Kernel::new();
        kernel
            .register_typed(crate::FileHandler::new(root.path()).unwrap())
            .await;
        let base_uri = ResourceUri::parse(&path.display().to_string()).unwrap();
        let baseline = kernel
            .execute_operation(Operation::Read(vec![crate::ReadRequest {
                uri: base_uri.clone(),
                at: None,
                before: None,
                after: None,
            }]))
            .await
            .unwrap();
        let OperationResult::Read(baseline_values) = baseline else {
            panic!("wrong baseline result")
        };
        let Ok(crate::ReadResult::Text(baseline_text)) = &baseline_values[0] else {
            panic!("wrong baseline value")
        };
        let anchor = baseline_text.lines[0].anchor.to_string();
        let uri = base_uri.with_fragment(anchor.trim_start_matches('#'));
        let result = kernel
            .execute_operation(Operation::Read(vec![crate::ReadRequest {
                uri,
                at: None,
                before: None,
                after: Some(0),
            }]))
            .await
            .unwrap();
        let OperationResult::Read(values) = result else {
            panic!("wrong result")
        };
        let Ok(crate::ReadResult::Text(text)) = &values[0] else {
            panic!("wrong value: {:?}", values[0])
        };
        assert_eq!(text.uri.fragment(), None);
        assert_eq!(text.lines.len(), 1);
        assert_eq!(text.lines[0].text, "first");

        let invalid = kernel
            .execute_operation(Operation::Write(vec![crate::WriteRequest {
                uri: base_uri.with_fragment(anchor.trim_start_matches('#')),
                content: "no mutation".to_owned(),
            }]))
            .await;
        assert!(matches!(
            invalid,
            Ok(OperationResult::Write(ref values))
                if matches!(values.first(), Some(Err(KernelError::InvalidRequest { .. })))
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "first\nsecond\n");
    }

    #[tokio::test]
    async fn invalid_fragment_only_rejects_its_write_item() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first.txt");
        let second = root.path().join("second.txt");
        std::fs::write(&first, "first\n").unwrap();
        std::fs::write(&second, "second\n").unwrap();
        let kernel = Kernel::new();
        kernel
            .register_typed(crate::FileHandler::new(root.path()).unwrap())
            .await;
        let first_uri = ResourceUri::parse(&first.display().to_string())
            .unwrap()
            .with_fragment("missing");
        let second_uri = ResourceUri::parse(&second.display().to_string()).unwrap();
        let result = kernel
            .execute_operation(Operation::Write(vec![
                crate::WriteRequest {
                    uri: first_uri,
                    content: "must not write\n".to_owned(),
                },
                crate::WriteRequest {
                    uri: second_uri,
                    content: "updated\n".to_owned(),
                },
            ]))
            .await
            .unwrap();
        let OperationResult::Write(values) = result else {
            panic!("wrong result")
        };
        assert!(matches!(values[0], Err(KernelError::InvalidRequest { .. })));
        assert!(values[1].is_ok());
        assert_eq!(std::fs::read_to_string(first).unwrap(), "first\n");
        assert_eq!(std::fs::read_to_string(second).unwrap(), "updated\n");
    }
}

fn operation_uri(operation: &Operation) -> String {
    match operation {
        Operation::Read(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Write(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Edit(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Run(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Send(items) => items.first().map(|item| item.uri.to_string()),
        Operation::Abort(items) | Operation::Delete(items) => {
            items.first().map(ToString::to_string)
        }
        Operation::Find(item) => item.roots.first().map(ToString::to_string),
        Operation::Grep(item) => match &item.source {
            crate::GrepSource::Resources(items) => items.first().map(ToString::to_string),
            crate::GrepSource::Text(items) => items.first().map(|item| item.uri.to_string()),
        },
        Operation::Poll(item) => item.targets.first().map(|item| item.uri.to_string()),
    }
    .unwrap_or_else(|| "<operation>".to_owned())
}

fn operation_verb(operation: &Operation) -> &'static str {
    match operation {
        Operation::Read(_) => "read",
        Operation::Write(_) => "write",
        Operation::Edit(_) => "edit",
        Operation::Find(_) => "find",
        Operation::Grep(_) => "grep",
        Operation::Run(_) => "run",
        Operation::Send(_) => "send",
        Operation::Abort(_) => "abort",
        Operation::Delete(_) => "delete",
        Operation::Poll(_) => "poll",
    }
}

fn poll_timeout(condition: &Option<crate::PollCondition>) -> Option<u64> {
    fn walk(condition: &crate::PollCondition, values: &mut Vec<u64>) {
        match condition {
            crate::PollCondition::Atom(crate::PollAtom::Timeout(milliseconds)) => {
                values.push(*milliseconds)
            }
            crate::PollCondition::Atom(_) => {}
            crate::PollCondition::All(children) | crate::PollCondition::Any(children) => {
                children.iter().for_each(|child| walk(child, values))
            }
        }
    }
    let mut values = Vec::new();
    condition
        .as_ref()
        .into_iter()
        .for_each(|condition| walk(condition, &mut values));
    values.into_iter().min()
}

fn evaluate_poll_condition(
    condition: Option<&crate::PollCondition>,
    results: &[crate::PollResult],
    elapsed: tokio::time::Duration,
) -> (bool, Vec<crate::PollAtom>) {
    fn atom(
        atom: &crate::PollAtom,
        results: &[crate::PollResult],
        elapsed: tokio::time::Duration,
    ) -> (bool, Vec<crate::PollAtom>) {
        let ok = match atom {
            crate::PollAtom::Changed(target) => results.get(*target as usize).is_some_and(|result| result.text.iter().any(|text| !text.lines.is_empty())),
            crate::PollAtom::Regex(regex) => results
                .get(regex.target as usize)
                .is_some_and(|result| {
                    regex::Regex::new(&regex.pattern).is_ok_and(|regex| {
                        regex.is_match(&crate::accumulated_lines_text(
                            result.text.iter().flat_map(|text| text.lines.iter()),
                        ))
                    })
                }),
            crate::PollAtom::Terminated(target) => results.get(*target as usize).is_some_and(|result| result.satisfied.iter().any(|item| matches!(item, crate::PollAtom::Terminated(found) if found == target))),
            crate::PollAtom::Timeout(milliseconds) => {
                elapsed >= tokio::time::Duration::from_millis(*milliseconds)
            }
        };
        (ok, ok.then(|| vec![atom.clone()]).unwrap_or_default())
    }
    fn walk(
        condition: &crate::PollCondition,
        results: &[crate::PollResult],
        elapsed: tokio::time::Duration,
    ) -> (bool, Vec<crate::PollAtom>) {
        match condition {
            crate::PollCondition::Atom(atom_value) => atom(atom_value, results, elapsed),
            crate::PollCondition::All(children) => {
                let values = children
                    .iter()
                    .map(|child| walk(child, results, elapsed))
                    .collect::<Vec<_>>();
                (
                    values.iter().all(|(ok, _)| *ok),
                    values.into_iter().flat_map(|(_, atoms)| atoms).collect(),
                )
            }
            crate::PollCondition::Any(children) => {
                let values = children
                    .iter()
                    .map(|child| walk(child, results, elapsed))
                    .collect::<Vec<_>>();
                (
                    values.iter().any(|(ok, _)| *ok),
                    values
                        .into_iter()
                        .filter(|(ok, _)| *ok)
                        .flat_map(|(_, atoms)| atoms)
                        .collect(),
                )
            }
        }
    }
    if condition.is_none() {
        let mut atoms = Vec::new();
        for (index, result) in results.iter().enumerate() {
            if result.text.iter().any(|text| !text.lines.is_empty()) {
                atoms.push(crate::PollAtom::Changed(index as u32));
            }
            if result.satisfied.iter().any(
                |atom| matches!(atom, crate::PollAtom::Terminated(found) if *found == index as u32),
            ) {
                atoms.push(crate::PollAtom::Terminated(index as u32));
            }
        }
        return (!atoms.is_empty(), atoms);
    }
    walk(
        condition.expect("condition checked above"),
        results,
        elapsed,
    )
}

/// URI router and universal operation dispatcher.
#[derive(Clone)]
pub struct Kernel {
    inner: Arc<Inner>,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                handlers: RwLock::new(Vec::new()),
                typed_handlers: RwLock::new(Vec::new()),
                tool_providers: RwLock::new(Vec::new()),
                resource_catalog_providers: RwLock::new(Vec::new()),
                background: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Retain a runtime-owned background service until the last kernel clone
    /// is dropped. Services can implement `Drop` to stop and join their
    /// worker cleanly; this is intentionally generic so the kernel does not
    /// depend on a particular watcher implementation.
    pub fn retain_background<T>(&self, service: T)
    where
        T: Send + 'static,
    {
        self.inner
            .background
            .lock()
            .unwrap()
            .push(Box::new(service));
    }

    pub async fn register_typed<H>(&self, handler: H)
    where
        H: TypedHandler + 'static,
    {
        self.inner
            .typed_handlers
            .write()
            .await
            .push(Arc::new(handler));
    }

    pub async fn register_typed_handler<H>(&self, handler: H)
    where
        H: Handler + TypedHandler + 'static,
    {
        let handler = Arc::new(handler);
        self.inner.handlers.write().await.push(handler.clone());
        self.inner.typed_handlers.write().await.push(handler);
    }

    /// Dispatches the typed contract surface. This is the component-native
    /// entry point; `execute(Request)` remains only as a native adapter for
    /// provider and CLI integrations.
    pub async fn execute_operation(
        &self,
        operation: Operation,
    ) -> Result<OperationResult, KernelError> {
        self.execute_operation_with_context(operation, InvocationContext::default())
            .await
    }

    pub async fn execute_operation_with_context(
        &self,
        operation: Operation,
        context: InvocationContext,
    ) -> Result<OperationResult, KernelError> {
        self.execute_operation_with_scope_internal(operation, InvocationScope::new(context))
            .await
    }

    async fn execute_operation_with_scope_internal(
        &self,
        operation: Operation,
        scope: InvocationScope,
    ) -> Result<OperationResult, KernelError> {
        let handlers = self.inner.typed_handlers.read().await;
        let host = self.handle();
        match operation {
            Operation::Read(requests) => {
                let mut output = Vec::with_capacity(requests.len());
                for request in requests {
                    let result =
                        match crate::lower_operation_fragments(Operation::Read(vec![request])) {
                            Ok(item) => {
                                self.execute_typed_item(
                                    &handlers,
                                    item,
                                    host.clone(),
                                    scope.clone(),
                                )
                                .await
                            }
                            Err(error) => Err(error),
                        };
                    match result {
                        Ok(OperationResult::Read(mut values)) => output.append(&mut values),
                        Ok(_) => output.push(Err(KernelError::Handler {
                            message: "typed read returned the wrong result variant".to_owned(),
                        })),
                        Err(error) => output.push(Err(error)),
                    }
                }
                Ok(OperationResult::Read(output))
            }
            Operation::Write(requests) => {
                let mut seen = std::collections::HashSet::with_capacity(requests.len());
                if let Some(duplicate) = requests.iter().find_map(|request| {
                    (!seen.insert(request.uri.clone())).then_some(request.uri.clone())
                }) {
                    let error = || KernelError::InvalidRequest {
                        message: format!("duplicate write URI: {duplicate}"),
                    };
                    return Ok(OperationResult::Write(
                        (0..requests.len()).map(|_| Err(error())).collect(),
                    ));
                }
                let mut output = Vec::with_capacity(requests.len());
                for request in requests {
                    let result =
                        match crate::lower_operation_fragments(Operation::Write(vec![request])) {
                            Ok(item) => {
                                self.execute_typed_item(
                                    &handlers,
                                    item,
                                    host.clone(),
                                    scope.clone(),
                                )
                                .await
                            }
                            Err(error) => Err(error),
                        };
                    match result {
                        Ok(OperationResult::Write(mut values)) => output.append(&mut values),
                        Ok(_) => output.push(Err(KernelError::Handler {
                            message: "typed write returned the wrong result variant".to_owned(),
                        })),
                        Err(error) => output.push(Err(error)),
                    }
                }
                Ok(OperationResult::Write(output))
            }
            Operation::Edit(requests) => {
                let mut output = Vec::with_capacity(requests.len());
                for request in requests {
                    let result =
                        match crate::lower_operation_fragments(Operation::Edit(vec![request])) {
                            Ok(item) => {
                                self.execute_typed_item(
                                    &handlers,
                                    item,
                                    host.clone(),
                                    scope.clone(),
                                )
                                .await
                            }
                            Err(error) => Err(error),
                        };
                    match result {
                        Ok(OperationResult::Edit(mut values)) => output.append(&mut values),
                        Ok(_) => output.push(Err(KernelError::Handler {
                            message: "typed edit returned the wrong result variant".to_owned(),
                        })),
                        Err(error) => output.push(Err(error)),
                    }
                }
                Ok(OperationResult::Edit(output))
            }
            Operation::Run(requests) => Ok(OperationResult::Run(
                self.route_run(&handlers, requests, host, scope).await,
            )),
            Operation::Send(requests) => Ok(OperationResult::Send(
                self.route_send(&handlers, requests, host, scope).await,
            )),
            Operation::Abort(uris) => Ok(OperationResult::Abort(
                self.route_uris(&handlers, uris, Operation::Abort, host, scope)
                    .await,
            )),
            Operation::Delete(uris) => Ok(OperationResult::Delete(
                self.route_uris(&handlers, uris, Operation::Delete, host, scope)
                    .await,
            )),
            Operation::Find(request) => {
                match crate::lower_operation_fragments(Operation::Find(request))? {
                    Operation::Find(request) => {
                        self.route_find(&handlers, request, host, scope).await
                    }
                    _ => unreachable!(),
                }
            }
            Operation::Grep(request) => {
                match crate::lower_operation_fragments(Operation::Grep(request))? {
                    Operation::Grep(request) => {
                        self.route_grep(&handlers, request, host, scope).await
                    }
                    _ => unreachable!(),
                }
            }
            Operation::Poll(request) => {
                match crate::lower_operation_fragments(Operation::Poll(request))? {
                    Operation::Poll(request) => {
                        self.route_poll(&handlers, request, host, scope).await
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    pub async fn execute_operation_with_scope(
        &self,
        operation: Operation,
        scope: InvocationScope,
    ) -> Result<OperationResult, KernelError> {
        // Keep the context-compatible routing implementation available while
        // handlers migrate to the scope-aware hook. The scope-aware handle is
        // still preserved across nested calls by the handler hook.
        self.execute_operation_with_scope_internal(operation, scope)
            .await
    }

    async fn execute_typed_item(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        operation: Operation,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> Result<OperationResult, KernelError> {
        let mut owners = Vec::new();
        for handler in handlers {
            let key = format!(
                "{}:{}",
                handler.descriptor().name,
                operation_claim_key(&operation)
            );
            let decision = if let Some(decision) = scope.pinned_claim(&key) {
                decision
            } else {
                let decision = handler
                    .claim_operation_with_scope(&operation, scope.clone())
                    .await?;
                scope.pin_claim(key, decision);
                decision
            };
            if decision != ClaimDecision::Pass {
                owners.push((handler, decision));
            }
        }
        if owners.len() > 1 {
            return Err(KernelError::Conflict {
                uri: operation_uri(&operation),
            });
        }
        let Some((handler, decision)) = owners.first() else {
            return Err(KernelError::UnsupportedVerb {
                verb: operation_verb(&operation).to_owned(),
                uri: operation_uri(&operation),
            });
        };
        if *decision == ClaimDecision::Reserve {
            return Err(KernelError::UnsupportedVerb {
                verb: operation_verb(&operation).to_owned(),
                uri: operation_uri(&operation),
            });
        }
        handler
            .execute_typed_with_scope(operation, host, scope)
            .await
    }

    async fn route_run(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        requests: Vec<crate::RunRequest>,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> Vec<Result<ResourceUri, KernelError>> {
        let mut output = Vec::with_capacity(requests.len());
        for request in requests {
            if request.uri.fragment().is_some() {
                output.push(Err(KernelError::InvalidRequest {
                    message: format!("fragment is not valid for run: {}", request.uri),
                }));
                continue;
            }
            match self
                .execute_typed_item(
                    handlers,
                    Operation::Run(vec![request]),
                    host.clone(),
                    scope.clone(),
                )
                .await
            {
                Ok(OperationResult::Run(mut values)) => output.push(values.remove(0)),
                Ok(_) => output.push(Err(KernelError::Handler {
                    message: "typed batch returned the wrong result variant".to_owned(),
                })),
                Err(error) => output.push(Err(error)),
            }
        }
        output
    }

    async fn route_send(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        requests: Vec<crate::SendRequest>,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> Vec<Result<ResourceUri, KernelError>> {
        let mut output = Vec::with_capacity(requests.len());
        for request in requests {
            if request.uri.fragment().is_some() {
                output.push(Err(KernelError::InvalidRequest {
                    message: format!("fragment is not valid for send: {}", request.uri),
                }));
                continue;
            }
            match self
                .execute_typed_item(
                    handlers,
                    Operation::Send(vec![request]),
                    host.clone(),
                    scope.clone(),
                )
                .await
            {
                Ok(OperationResult::Send(mut values)) => output.push(values.remove(0)),
                Ok(_) => output.push(Err(KernelError::Handler {
                    message: "typed send returned the wrong result variant".to_owned(),
                })),
                Err(error) => output.push(Err(error)),
            }
        }
        output
    }

    async fn route_uris(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        uris: Vec<ResourceUri>,
        verb: impl Fn(Vec<ResourceUri>) -> Operation,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> Vec<Result<ResourceUri, KernelError>> {
        let mut output = Vec::with_capacity(uris.len());
        for uri in uris {
            if uri.fragment().is_some() {
                output.push(Err(KernelError::InvalidRequest {
                    message: format!("fragment is not valid for URI operation: {uri}"),
                }));
                continue;
            }
            match self
                .execute_typed_item(
                    handlers,
                    verb(vec![uri.clone()]),
                    host.clone(),
                    scope.clone(),
                )
                .await
            {
                Ok(OperationResult::Abort(mut values))
                | Ok(OperationResult::Delete(mut values)) => output.push(values.remove(0)),
                Ok(_) => output.push(Err(KernelError::Handler {
                    message: "typed URI batch returned the wrong result variant".to_owned(),
                })),
                Err(error) => output.push(Err(error)),
            }
        }
        output
    }

    async fn route_find(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        request: crate::FindRequest,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> Result<OperationResult, KernelError> {
        let mut paths = Vec::new();
        for root in request.roots {
            match self
                .execute_typed_item(
                    handlers,
                    Operation::Find(crate::FindRequest {
                        roots: vec![root],
                        query: request.query.clone(),
                    }),
                    host.clone(),
                    scope.clone(),
                )
                .await?
            {
                OperationResult::Find(result) => paths.extend(result?),
                _ => {
                    return Err(KernelError::Handler {
                        message: "typed find returned the wrong result variant".to_owned(),
                    });
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        paths.retain(|path| seen.insert(path.clone()));
        Ok(OperationResult::Find(Ok(paths)))
    }

    async fn route_grep(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        request: crate::GrepRequest,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> Result<OperationResult, KernelError> {
        match request.source {
            crate::GrepSource::Resources(uris) => {
                let mut text = Vec::new();
                for uri in uris {
                    match self
                        .execute_typed_item(
                            handlers,
                            Operation::Grep(crate::GrepRequest {
                                pattern: request.pattern.clone(),
                                source: crate::GrepSource::Resources(vec![uri]),
                            }),
                            host.clone(),
                            scope.clone(),
                        )
                        .await?
                    {
                        OperationResult::Grep(result) => text.extend(result?),
                        _ => {
                            return Err(KernelError::Handler {
                                message: "typed grep returned the wrong result variant".to_owned(),
                            });
                        }
                    }
                }
                Ok(OperationResult::Grep(Ok(text)))
            }
            source => {
                self.execute_typed_item(
                    handlers,
                    Operation::Grep(crate::GrepRequest {
                        pattern: request.pattern,
                        source,
                    }),
                    host,
                    scope,
                )
                .await
            }
        }
    }

    async fn route_poll(
        &self,
        handlers: &[Arc<dyn TypedHandler>],
        request: crate::PollRequest,
        host: KernelHandle,
        scope: InvocationScope,
    ) -> Result<OperationResult, KernelError> {
        if request.targets.is_empty() {
            return Err(KernelError::InvalidRequest {
                message: "poll requires at least one target".to_owned(),
            });
        }
        for target in &request.targets {
            let operation = Operation::Poll(crate::PollRequest {
                targets: vec![target.clone()],
                until: None,
            });
            let mut owner = None;
            for (index, handler) in handlers.iter().enumerate() {
                let decision = handler
                    .claim_operation_with_scope(&operation, scope.clone())
                    .await?;
                if decision == ClaimDecision::Pass {
                    continue;
                }
                if owner.replace((index, decision)).is_some() {
                    return Err(KernelError::Conflict {
                        uri: target.uri.to_string(),
                    });
                }
            }
            let Some((_index, decision)) = owner else {
                return Err(KernelError::NoHandler {
                    uri: target.uri.to_string(),
                });
            };
            if decision == ClaimDecision::Reserve {
                return Err(KernelError::UnsupportedVerb {
                    verb: "poll".to_owned(),
                    uri: target.uri.to_string(),
                });
            }
        }
        let started = tokio::time::Instant::now();
        let timeout_ms = poll_timeout(&request.until);
        loop {
            let slice_ms = timeout_ms
                .map(|ms| {
                    ms.saturating_sub(started.elapsed().as_millis() as u64)
                        .min(50)
                })
                .unwrap_or(50);
            let futures = request.targets.iter().map(|target| {
                let operation = Operation::Poll(crate::PollRequest {
                    targets: vec![target.clone()],
                    until: Some(crate::PollCondition::Atom(crate::PollAtom::Timeout(
                        slice_ms,
                    ))),
                });
                self.execute_typed_item(handlers, operation, host.clone(), scope.clone())
            });
            let mut results = Vec::with_capacity(request.targets.len());
            for result in futures::future::join_all(futures).await {
                let OperationResult::Poll(result) = result? else {
                    return Err(KernelError::Handler {
                        message: "typed poll returned the wrong result variant".to_owned(),
                    });
                };
                results.push(result?);
            }
            let elapsed = started.elapsed();
            let (satisfied, atoms) =
                evaluate_poll_condition(request.until.as_ref(), &results, elapsed);
            if satisfied {
                return Ok(OperationResult::Poll(Ok(crate::PollResult {
                    text: results.into_iter().flat_map(|result| result.text).collect(),
                    satisfied: atoms,
                })));
            }
        }
    }

    pub async fn register<H>(&self, handler: H)
    where
        H: Handler + 'static,
    {
        self.inner.handlers.write().await.push(Arc::new(handler));
    }

    /// Register one object as both a URI handler and a named-tool provider.
    /// Keeping the two trait objects backed by the same allocation is
    /// important for self-modifying handlers: their catalog and executor
    /// must observe the same active component generations.
    pub async fn register_tool_handler<H>(&self, handler: H)
    where
        H: Handler + ToolProvider + 'static,
    {
        let handler = Arc::new(handler);
        self.inner.handlers.write().await.push(handler.clone());
        self.inner.tool_providers.write().await.push(handler);
    }

    /// Register one allocation across all three compatible surfaces. This is
    /// important for self-modifying handlers: typed filesystem edits, native
    /// URI access, and named-tool execution must share the same registry,
    /// dirty set, and active-generation pins.
    pub async fn register_typed_tool_handler<H>(&self, handler: H)
    where
        H: Handler + TypedHandler + ToolProvider + 'static,
    {
        let handler = Arc::new(handler);
        self.inner.handlers.write().await.push(handler.clone());
        self.inner
            .typed_handlers
            .write()
            .await
            .push(handler.clone());
        self.inner.tool_providers.write().await.push(handler);
    }

    /// Register a typed resource handler and expose its active-generation
    /// documentation to provider adapters using the same allocation.
    pub async fn register_typed_resource_handler<H>(&self, handler: H)
    where
        H: Handler + TypedHandler + ResourceCatalogProvider + 'static,
    {
        let handler = Arc::new(handler);
        self.inner.handlers.write().await.push(handler.clone());
        self.inner
            .typed_handlers
            .write()
            .await
            .push(handler.clone());
        self.inner
            .resource_catalog_providers
            .write()
            .await
            .push(handler);
    }

    pub async fn register_tool_provider<P>(&self, provider: P)
    where
        P: ToolProvider + 'static,
    {
        self.inner
            .tool_providers
            .write()
            .await
            .push(Arc::new(provider));
    }

    pub async fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.inner
            .tool_providers
            .read()
            .await
            .iter()
            .flat_map(|provider| provider.tool_definitions())
            .collect()
    }

    pub async fn resource_catalog(&self) -> Vec<ResourceCatalogEntry> {
        self.inner
            .resource_catalog_providers
            .read()
            .await
            .iter()
            .flat_map(|provider| provider.resource_catalog())
            .collect()
    }

    pub async fn execute_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, KernelError> {
        self.execute_tool_with_context(name, args, InvocationContext::default())
            .await
    }

    pub async fn execute_tool_with_context(
        &self,
        name: &str,
        args: serde_json::Value,
        context: InvocationContext,
    ) -> Result<serde_json::Value, KernelError> {
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            let definitions = provider.tool_definitions();
            if definitions.iter().any(|definition| definition.name == name) {
                return provider
                    .execute_tool_with_context(name, args, host, context)
                    .await;
            }
        }
        Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        })
    }

    pub async fn execute_tool_with_scope(
        &self,
        name: &str,
        args: serde_json::Value,
        scope: InvocationScope,
    ) -> Result<serde_json::Value, KernelError> {
        let providers = self.inner.tool_providers.read().await;
        let host = self.handle();
        for provider in providers.iter() {
            let definitions = provider.tool_definitions();
            if definitions.iter().any(|definition| definition.name == name) {
                return provider
                    .execute_tool_with_scope(name, args, host, scope)
                    .await;
            }
        }
        Err(KernelError::Handler {
            message: format!("no named tool registered: {name}"),
        })
    }

    pub async fn descriptors(&self) -> Vec<HandlerDescriptor> {
        self.inner
            .handlers
            .read()
            .await
            .iter()
            .map(|handler| handler.descriptor())
            .collect()
    }

    pub fn handle(&self) -> KernelHandle {
        let kernel = self.clone();
        let typed_kernel = kernel.clone();
        KernelHandle::new(
            Arc::new(move |request| {
                let kernel = kernel.clone();
                Box::pin(async move { kernel.execute(request).await })
            }),
            Arc::new(move |operation, scope| {
                let kernel = typed_kernel.clone();
                // Every host-mediated nested call gets a descendant token:
                // parent cancellation still propagates inward, while a
                // child invocation cannot cancel its caller.
                Box::pin(async move {
                    kernel
                        .execute_operation_with_scope(operation, scope.child())
                        .await
                })
            }),
        )
    }

    pub async fn execute(&self, request: Request) -> ItemResult {
        let request = match crate::normalize(&request.target) {
            Ok(target) => Request { target, ..request },
            Err(error) => return ItemResult::failure(request.target, error),
        };
        let target = request.target.clone();
        let handlers = self.inner.handlers.read().await;
        let Some(handler) = handlers.iter().find(|handler| handler.claims(&target)) else {
            return ItemResult::failure(
                target.clone(),
                KernelError::NoHandler {
                    uri: target.to_string(),
                },
            );
        };
        if !handler.supports(request.verb) {
            return ItemResult::failure(
                target.clone(),
                KernelError::UnsupportedVerb {
                    verb: request.verb.to_string(),
                    uri: target.to_string(),
                },
            );
        }
        let host = self.handle();
        let result = handler.execute(request, host).await;
        match result {
            Ok(value) => ItemResult::success(target, value),
            Err(error) => ItemResult::failure(target, error),
        }
    }

    pub async fn execute_batch(&self, batch: BatchRequest) -> BatchResult {
        let mut items = Vec::with_capacity(batch.items.len());
        for request in batch.items {
            items.push(self.execute(request).await);
        }
        BatchResult { items }
    }

    pub async fn registered_verbs(&self) -> Vec<Verb> {
        let mut verbs = self
            .descriptors()
            .await
            .into_iter()
            .flat_map(|descriptor| descriptor.verbs)
            .collect::<Vec<_>>();
        verbs.sort_by_key(|verb| verb.to_string());
        verbs.dedup();
        verbs
    }
}

fn operation_claim_key(operation: &Operation) -> String {
    format!("{}:{}", operation_verb(operation), operation_uri(operation))
}
