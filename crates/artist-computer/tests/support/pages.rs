//! A local origin for the CDP tests.
//!
//! The existing CDP tests run against `file://`, which is enough to prove the
//! protocol works and not enough to prove anything else: `fetch` does not work
//! there, an iframe from a different path is a different origin, and nothing
//! ever loads slowly. Every hard part of driving a real page — content that
//! arrives after a click, a control inside a frame, a navigation that starts
//! late — needs a real `http://` origin to exist at all.
//!
//! So: a static server, thirty lines, no dependency. It serves the fixtures in
//! `tests/pages/` and one deliberately slow JSON endpoint, because "the settle
//! predicate waits for the network" is untestable against something that
//! answers instantly.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// A server on a loopback port the OS chose.
pub struct Pages {
    pub origin: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Pages {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// How long `/slow.json` takes to answer.
///
/// Long enough that a predicate which samples immediately after a click will
/// see the *old* page — which is the bug worth catching — and short enough that
/// a test waiting for it is not slow.
const SLOW_MS: u64 = 400;

impl Pages {
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let served = std::sync::Arc::clone(&stop);

        std::thread::spawn(move || {
            for connection in listener.incoming() {
                if served.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                if let Ok(connection) = connection {
                    std::thread::spawn(move || {
                        let _ = serve(connection);
                    });
                }
            }
        });

        Ok(Self {
            origin: format!("http://127.0.0.1:{port}"),
            stop,
        })
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.origin)
    }
}

fn serve(mut connection: TcpStream) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new(&connection).read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let path = path.trim_start_matches('/');

    let (content_type, body) = if path == "slow.json" {
        // The point of the whole fixture: content that arrives late.
        std::thread::sleep(std::time::Duration::from_millis(SLOW_MS));
        (
            "application/json",
            br#"{"orders":["Order 4471","Order 4472","Order 4473"]}"#.to_vec(),
        )
    } else {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/pages")
            .join(if path.is_empty() { "spa.html" } else { path });
        match std::fs::read(&file) {
            Ok(body) => ("text/html; charset=utf-8", body),
            Err(_) => ("text/plain", b"not found".to_vec()),
        }
    };

    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    connection.write_all(head.as_bytes())?;
    connection.write_all(&body)?;
    connection.flush()
}
