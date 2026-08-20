//! Async NDJSON server adapter for the daemon protocol.

use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

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
        let frame = match RpcFrame::from_line(&line) {
            Ok(frame) => frame,
            Err(error) => {
                let response =
                    RpcFrame::error("invalid-request", "invalid_frame", error.to_string());
                writer.write_all(response.to_line()?.as_bytes()).await?;
                writer.flush().await?;
                continue;
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
                continue;
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
    }
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
