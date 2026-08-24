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
                cursor,
            } => {
                let regex = pattern
                    .map(|p| {
                        Regex::new(&p)
                            .map_err(|e| ResourceError::Invalid(format!("invalid poll regex: {e}")))
                    })
                    .transpose()?;
                let start = match cursor {
                    Some(cursor) => parse_cursor(&cursor, &self.state.lock().unwrap().text)?,
                    None => self.state.lock().unwrap().text.len(),
                };
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
                let next_cursor = format_cursor(start + text.len());
                Ok(ResourceReply::Poll {
                    text,
                    outcome,
                    next_cursor,
                })
            }
            other => Err(ResourceError::Unsupported {
                uri: other.uri().clone(),
                operation: other.operation(),
            }),
        }
    }
}

fn format_cursor(offset: usize) -> String {
    format!("append-v1:{offset}")
}

fn parse_cursor(cursor: &str, text: &str) -> Result<usize, ResourceError> {
    let offset = cursor
        .strip_prefix("append-v1:")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|offset| *offset <= text.len() && text.is_char_boundary(*offset))
        .ok_or_else(|| ResourceError::Invalid("invalid or stale poll cursor".into()))?;
    Ok(offset)
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
                        cursor: None,
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
                outcome: PollOutcome::Matched,
                next_cursor: "append-v1:13".into(),
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
                        cursor: None,
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
                        cursor: None,
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
                outcome: PollOutcome::TimedOut,
                next_cursor: "append-v1:5".into(),
            }
        );
    }

    #[tokio::test]
    async fn continuation_delivers_output_emitted_between_polls_exactly_once() {
        let resource = AppendOnlyResource::default();
        let uri = ResourceUri::resolve("process://run/out", Path::new("/")).unwrap();
        let first = resource
            .handle(ResourceRequest::Poll {
                uri: uri.clone(),
                pattern: None,
                timeout: Some(Duration::from_millis(1)),
                cursor: None,
            })
            .await
            .unwrap();
        let ResourceReply::Poll { next_cursor, .. } = first else {
            panic!("expected poll reply");
        };
        resource.append("between");
        let second = resource
            .handle(ResourceRequest::Poll {
                uri: uri.clone(),
                pattern: None,
                timeout: Some(Duration::from_millis(1)),
                cursor: Some(next_cursor),
            })
            .await
            .unwrap();
        let ResourceReply::Poll {
            text, next_cursor, ..
        } = second
        else {
            panic!("expected poll reply");
        };
        assert_eq!(text, "between");
        resource.append("after");
        let third = resource
            .handle(ResourceRequest::Poll {
                uri,
                pattern: None,
                timeout: Some(Duration::from_millis(1)),
                cursor: Some(next_cursor),
            })
            .await
            .unwrap();
        assert!(matches!(third, ResourceReply::Poll { text, .. } if text == "after"));
    }
}
