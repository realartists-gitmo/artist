use std::sync::Mutex;

use async_trait::async_trait;
use regex::Regex;
use tokio::sync::Notify;

use crate::{PollOutcome, ResourceError, ResourceProvider, ResourceReply, ResourceRequest};

/// A text resource useful for process output and REPL streams.
#[derive(Default)]
pub struct AppendOnlyResource {
    state: Mutex<State>,
    changed: Notify,
}
#[derive(Default)]
struct State {
    text: String,
    closed: bool,
}

impl AppendOnlyResource {
    pub fn append(&self, text: &str) {
        self.state.lock().unwrap().text.push_str(text);
        self.changed.notify_waiters();
    }
    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.changed.notify_waiters();
    }
}

#[async_trait]
impl ResourceProvider for AppendOnlyResource {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        match request {
            ResourceRequest::Read {
                start_line,
                line_count,
                ..
            } => {
                let text = self.state.lock().unwrap().text.clone();
                let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
                Ok(ResourceReply::Text {
                    text: text
                        .split_inclusive('\n')
                        .skip(start)
                        .take(line_count.unwrap_or(u64::MAX) as usize)
                        .collect(),
                })
            }
            ResourceRequest::Poll {
                uri: _,
                pattern,
                timeout,
            } => {
                let regex = pattern
                    .map(|p| {
                        Regex::new(&p)
                            .map_err(|e| ResourceError::Invalid(format!("invalid poll regex: {e}")))
                    })
                    .transpose()?;
                let start = self.state.lock().unwrap().text.len();
                let wait = async {
                    loop {
                        let notified = self.changed.notified();
                        {
                            let state = self.state.lock().unwrap();
                            let appended = &state.text[start..];
                            if regex.as_ref().is_some_and(|r| r.is_match(appended)) {
                                return (appended.to_owned(), PollOutcome::Matched);
                            }
                            if state.closed {
                                return (appended.to_owned(), PollOutcome::Closed);
                            }
                        }
                        notified.await;
                    }
                };
                let (text, outcome) = match timeout {
                    Some(duration) => match tokio::time::timeout(duration, wait).await {
                        Ok(result) => result,
                        Err(_) => (
                            self.state.lock().unwrap().text[start..].to_owned(),
                            PollOutcome::TimedOut,
                        ),
                    },
                    None => wait.await,
                };
                Ok(ResourceReply::Poll { text, outcome })
            }
            other => Err(ResourceError::Unsupported {
                uri: other.uri().clone(),
                operation: other.operation(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceUri;
    use std::{path::Path, sync::Arc, time::Duration};

    #[tokio::test]
    async fn poll_sees_only_subsequent_text_and_matches() {
        let resource = Arc::new(AppendOnlyResource::default());
        resource.append("old\n");
        let pending = {
            let resource = resource.clone();
            tokio::spawn(async move {
                resource
                    .handle(ResourceRequest::Poll {
                        uri: ResourceUri::resolve("process://run/out", Path::new("/")).unwrap(),
                        pattern: Some("done".into()),
                        timeout: Some(Duration::from_secs(1)),
                    })
                    .await
                    .unwrap()
            })
        };
        tokio::task::yield_now().await;
        resource.append("new done\n");
        assert_eq!(
            pending.await.unwrap(),
            ResourceReply::Poll {
                text: "new done\n".into(),
                outcome: PollOutcome::Matched
            }
        );
    }

    #[tokio::test]
    async fn poll_without_regex_waits_for_close() {
        let resource = Arc::new(AppendOnlyResource::default());
        let pending = {
            let resource = resource.clone();
            tokio::spawn(async move {
                resource
                    .handle(ResourceRequest::Poll {
                        uri: ResourceUri::resolve("process://run/out", Path::new("/")).unwrap(),
                        pattern: None,
                        timeout: Some(Duration::from_secs(1)),
                    })
                    .await
                    .unwrap()
            })
        };
        tokio::task::yield_now().await;
        resource.append("text");
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        resource.close();
        assert!(matches!(
            pending.await.unwrap(),
            ResourceReply::Poll {
                outcome: PollOutcome::Closed,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn poll_times_out_with_appended_text() {
        let resource = Arc::new(AppendOnlyResource::default());
        let pending = {
            let resource = resource.clone();
            tokio::spawn(async move {
                resource
                    .handle(ResourceRequest::Poll {
                        uri: ResourceUri::resolve("process://run/out", Path::new("/")).unwrap(),
                        pattern: None,
                        timeout: Some(Duration::from_millis(20)),
                    })
                    .await
                    .unwrap()
            })
        };
        tokio::task::yield_now().await;
        resource.append("later");
        assert_eq!(
            pending.await.unwrap(),
            ResourceReply::Poll {
                text: "later".into(),
                outcome: PollOutcome::TimedOut
            }
        );
    }
}
