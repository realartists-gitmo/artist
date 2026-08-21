//! Async NDJSON server adapter for the daemon protocol.

use std::collections::BTreeSet;
use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

use crate::protocol::{FrameError, RpcError, RpcFrame};

#[derive(Debug, thiserror::Error)]
pub enum RpcServerError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Frame(#[from] FrameError),
}

/// Domain handler used by every NDJSON transport.
#[async_trait]
pub trait RpcHandler: Send + Sync {
    async fn handle(&self, request: RpcFrame) -> Result<Vec<RpcFrame>, RpcError>;
}

/// Optional live event source for transports that keep a connection open
/// while a session turn runs. Events are deliberately transport-neutral and
/// retain the same stream/sequence identity used by replay.
pub trait RpcEventSource: Send + Sync {
    fn subscribe_events(&self) -> broadcast::Receiver<RpcFrame>;

    /// Return the durable stream IDs this connection has requested. The
    /// transport keeps the host-owned event bus shared, but never forwards a
    /// different session's live event to a client that has not addressed it.
    fn event_stream_ids(&self, _request: &RpcFrame) -> Vec<String> {
        Vec::new()
    }
}

/// Serve newline-delimited frames until the input closes.
pub async fn serve_ndjson<R, W, H>(
    reader: R,
    mut writer: W,
    handler: &H,
) -> Result<(), RpcServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    H: RpcHandler,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        process_line(&line, &mut writer, handler).await?;
    }
    Ok(())
}

/// Variant used by the daemon executable. Request processing remains
/// connection-independent while the same writer also receives live session
/// events from the host-owned event bus.
pub async fn serve_ndjson_with_events<R, W, H>(
    reader: R,
    mut writer: W,
    handler: &H,
) -> Result<(), RpcServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    H: RpcHandler + RpcEventSource,
{
    let mut lines = BufReader::new(reader).lines();
    let mut events = handler.subscribe_events();
    let mut subscribed_streams = BTreeSet::new();
    let mut event_source_open = true;
    loop {
        if event_source_open {
            tokio::select! {
                line = lines.next_line() => {
                    let Some(line) = line? else { break };
                    if let Ok(frame) = RpcFrame::from_line(&line) {
                        subscribed_streams.extend(handler.event_stream_ids(&frame));
                    }
                    process_line(&line, &mut writer, handler).await?;
                }
                event = events.recv() => {
                    match event {
                        Ok(frame) if event_is_subscribed(&frame, &subscribed_streams) => {
                                writer.write_all(frame.to_line()?.as_bytes()).await?;
                                writer.flush().await?;
                            }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => event_source_open = false,
                    }
                }
            }
        } else {
            let Some(line) = lines.next_line().await? else {
                break;
            };
            if let Ok(frame) = RpcFrame::from_line(&line) {
                subscribed_streams.extend(handler.event_stream_ids(&frame));
            }
            process_line(&line, &mut writer, handler).await?;
        }
    }
    Ok(())
}

fn event_is_subscribed(frame: &RpcFrame, streams: &BTreeSet<String>) -> bool {
    matches!(frame, RpcFrame::Event { stream_id, .. } if streams.contains(stream_id))
}

async fn process_line<W, H>(line: &str, writer: &mut W, handler: &H) -> Result<(), RpcServerError>
where
    W: AsyncWrite + Unpin,
    H: RpcHandler,
{
    let frame = match RpcFrame::from_line(line) {
        Ok(frame) => frame,
        Err(error) => {
            let response = RpcFrame::error("invalid-request", "invalid_frame", error.to_string());
            writer.write_all(response.to_line()?.as_bytes()).await?;
            writer.flush().await?;
            return Ok(());
        }
    };
    let request_id = match &frame {
        RpcFrame::Request { id, .. } => id.clone(),
        _ => {
            let response = RpcFrame::error(
                "invalid-request",
                "request_required",
                "expected a request frame",
            );
            writer.write_all(response.to_line()?.as_bytes()).await?;
            writer.flush().await?;
            return Ok(());
        }
    };
    match handler.handle(frame).await {
        Ok(frames) => {
            for frame in frames {
                writer.write_all(frame.to_line()?.as_bytes()).await?;
            }
        }
        Err(error) => {
            writer
                .write_all(
                    RpcFrame::Response {
                        id: request_id,
                        result: None,
                        error: Some(error),
                    }
                    .to_line()?
                    .as_bytes(),
                )
                .await?;
        }
    }
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Echo;

    #[async_trait]
    impl RpcHandler for Echo {
        async fn handle(&self, request: RpcFrame) -> Result<Vec<RpcFrame>, RpcError> {
            let RpcFrame::Request { id, params, .. } = request else {
                unreachable!()
            };
            Ok(vec![RpcFrame::response(id, params)])
        }
    }

    #[tokio::test]
    async fn serves_requests_and_closes_cleanly() {
        let (mut client, server_io) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            serve_ndjson(server_read, server_write, &Echo)
                .await
                .unwrap();
        });
        client
            .write_all(b"{\"kind\":\"request\",\"id\":\"1\",\"method\":\"ping\",\"params\":{\"ok\":true}}\n")
            .await
            .unwrap();
        let mut response = vec![0; 256];
        let size = client.read(&mut response).await.unwrap();
        let response =
            RpcFrame::from_line(std::str::from_utf8(&response[..size]).unwrap()).unwrap();
        assert_eq!(
            response,
            RpcFrame::response("1", serde_json::json!({"ok": true}))
        );
        drop(client);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_input_returns_a_protocol_error() {
        let (mut client, server_io) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server_io);
        let server = tokio::spawn(async move {
            serve_ndjson(server_read, server_write, &Echo)
                .await
                .unwrap();
        });
        client.write_all(b"not-json\n").await.unwrap();
        let mut response = vec![0; 256];
        let size = client.read(&mut response).await.unwrap();
        let response =
            RpcFrame::from_line(std::str::from_utf8(&response[..size]).unwrap()).unwrap();
        assert!(matches!(
            response,
            RpcFrame::Response { error: Some(_), .. }
        ));
        drop(client);
        server.await.unwrap();
    }
}
