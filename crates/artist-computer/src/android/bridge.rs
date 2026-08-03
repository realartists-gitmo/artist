//! The host side of the accessibility bridge.
//!
//! Speaks newline-delimited JSON to the resident `AccessibilityService` running
//! inside the container (`android-service/`), reached through `adb forward` on a
//! loopback port. Two kinds of message share the connection: replies, correlated
//! by an id we assign, and unsolicited **events** the service pushes when window
//! content changes. The events are the reason this is a socket rather than a
//! series of `adb shell` calls — they are the settle signal for rung 2, and a
//! request/response transport cannot carry them.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, broadcast, oneshot};

use crate::android::adb::Adb;
use crate::program::StepError;

/// The port the service listens on inside the container.
const DEVICE_PORT: u16 = 8722;

/// How long to wait for one reply.
///
/// A tree walk over a large app is tens of milliseconds; anything approaching
/// this means the service is wedged, and reporting that beats waiting.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// The package the service lives in, and the component Android knows it by.
pub const BRIDGE_PACKAGE: &str = "dev.artist.bridge";
pub const BRIDGE_COMPONENT: &str = "dev.artist.bridge/dev.artist.bridge.BridgeService";

/// One node as the service reported it.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct BridgeNode {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default, rename = "class")]
    pub class: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default, rename = "resId")]
    pub res_id: String,
    #[serde(default)]
    pub hint: String,
    /// `[left, top, right, bottom]`, in screen coordinates.
    #[serde(default)]
    pub bounds: Vec<i32>,
    #[serde(default)]
    pub clickable: bool,
    #[serde(default, rename = "longClickable")]
    pub long_clickable: bool,
    #[serde(default)]
    pub scrollable: bool,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub checkable: bool,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub visible: bool,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Tree {
    pub generation: u64,
    pub nodes: Vec<BridgeNode>,
    #[serde(default)]
    pub truncated: bool,
}

/// A connection to the in-container service.
pub struct Bridge {
    writer: Mutex<tokio::net::tcp::OwnedWriteHalf>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    /// Content-changed notifications, for settle watchers.
    events: broadcast::Sender<()>,
    next_id: AtomicU64,
}

impl Bridge {
    /// Forward the port and connect.
    ///
    /// The host port is chosen by adb rather than fixed: a fixed one collides
    /// with a second agent, and with whatever else on the machine happens to
    /// want it.
    pub async fn connect(adb: &Adb) -> Result<Self, StepError> {
        let forwarded = forward(adb).await?;
        let stream = TcpStream::connect(("127.0.0.1", forwarded))
            .await
            .map_err(|error| {
                StepError::Backend(format!(
                    "the accessibility bridge is not answering on 127.0.0.1:{forwarded}: {error}. \
                     Is it installed and enabled? `artist computer doctor` will say."
                ))
            })?;
        stream.set_nodelay(true).ok();

        let (read, write) = stream.into_split();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> = Arc::default();
        let (events, _) = broadcast::channel(64);

        // One reader task owns the socket. Replies are matched by id and events
        // are fanned out; anything unrecognized is dropped rather than treated
        // as a reply to whatever asked last, which would hand one caller another
        // caller's answer.
        let sink = Arc::clone(&pending);
        let notifier = events.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if value.get("event").is_some() {
                    let _ = notifier.send(());
                    continue;
                }
                let Some(id) = value.get("id").and_then(serde_json::Value::as_u64) else {
                    continue;
                };
                if let Some(waiting) = sink.lock().await.remove(&id) {
                    let _ = waiting.send(value);
                }
            }
        });

        Ok(Self {
            writer: Mutex::new(write),
            pending,
            events,
            next_id: AtomicU64::new(1),
        })
    }

    /// Subscribe to content-changed notifications.
    pub fn events(&self) -> broadcast::Receiver<()> {
        self.events.subscribe()
    }

    async fn request(&self, body: serde_json::Value) -> Result<serde_json::Value, StepError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = body;
        message["id"] = serde_json::json!(id);

        let (tx, rx) = oneshot::channel();
        // Registered *before* the write. Registering afterwards is a race the
        // service is fast enough to win: a reply can arrive before the sender is
        // in the map, and then nothing is ever woken.
        self.pending.lock().await.insert(id, tx);

        let line = format!("{message}\n");
        {
            let mut writer = self.writer.lock().await;
            writer.write_all(line.as_bytes()).await.map_err(|error| {
                StepError::Backend(format!("the accessibility bridge went away: {error}"))
            })?;
            writer.flush().await.ok();
        }

        let value = match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => {
                return Err(StepError::Backend(
                    "the accessibility bridge closed while a request was outstanding".into(),
                ));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(StepError::Backend(format!(
                    "the accessibility bridge did not answer within {}s",
                    REPLY_TIMEOUT.as_secs()
                )));
            }
        };

        if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
            return Err(StepError::Backend(error.to_owned()));
        }
        Ok(value)
    }

    pub async fn ping(&self) -> Result<(), StepError> {
        self.request(serde_json::json!({"op": "ping"})).await?;
        Ok(())
    }

    pub async fn tree(&self) -> Result<Tree, StepError> {
        let value = self.request(serde_json::json!({"op": "tree"})).await?;
        serde_json::from_value(value).map_err(|error| {
            StepError::Backend(format!(
                "the accessibility bridge sent a tree we could not read: {error}"
            ))
        })
    }

    pub async fn act(&self, node: &str, action: &str) -> Result<(), StepError> {
        self.request(serde_json::json!({"op": "act", "node": node, "action": action}))
            .await?;
        Ok(())
    }

    pub async fn set_text(&self, node: &str, text: &str) -> Result<(), StepError> {
        self.request(serde_json::json!({"op": "setText", "node": node, "text": text}))
            .await?;
        Ok(())
    }
}

/// Ask adb for a host port forwarded to the service's device port.
async fn forward(adb: &Adb) -> Result<u16, StepError> {
    let output = tokio::process::Command::new("adb")
        .args([
            "-s",
            adb.serial(),
            "forward",
            "tcp:0",
            &format!("tcp:{DEVICE_PORT}"),
        ])
        .output()
        .await
        .map_err(|error| StepError::Backend(format!("could not run adb forward: {error}")))?;

    if !output.status.success() {
        return Err(StepError::Backend(format!(
            "adb forward failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    // `tcp:0` makes adb pick a free port and print it.
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| {
            StepError::Backend(format!(
                "adb forward did not report a port: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            ))
        })
}

/// Install the bridge and turn it on.
///
/// Enabling is a *secure* setting, which is why this needs adb rather than being
/// something the APK can do for itself: Android deliberately makes granting
/// accessibility access an act performed from outside the app asking for it.
pub async fn install(adb: &Adb, apk: &std::path::Path) -> Result<(), StepError> {
    adb.install(apk).await?;

    // Appended rather than assigned. Overwriting the setting would silently
    // disable every other accessibility service the user has running — a screen
    // reader among them, on a container somebody actually uses.
    let existing = adb
        .shell(&[
            "settings",
            "get",
            "secure",
            "enabled_accessibility_services",
        ])
        .await
        .unwrap_or_default();
    let existing = existing.trim();
    let existing = if existing == "null" { "" } else { existing };

    if !existing.split(':').any(|entry| entry == BRIDGE_COMPONENT) {
        let combined = if existing.is_empty() {
            BRIDGE_COMPONENT.to_owned()
        } else {
            format!("{existing}:{BRIDGE_COMPONENT}")
        };
        adb.shell(&[
            "settings",
            "put",
            "secure",
            "enabled_accessibility_services",
            &combined,
        ])
        .await?;
    }
    adb.shell(&["settings", "put", "secure", "accessibility_enabled", "1"])
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_parses_from_what_the_service_sends() {
        let node: BridgeNode = serde_json::from_str(
            r#"{"id":"3:12","parent":"3:4","class":"android.widget.Button","package":"com.a",
                "text":"Save","desc":"","resId":"com.a:id/save","hint":"",
                "bounds":[10,20,110,70],"clickable":true,"longClickable":false,
                "scrollable":false,"editable":false,"checkable":false,"checked":false,
                "enabled":true,"focused":false,"selected":false,"visible":true}"#,
        )
        .unwrap();
        assert_eq!(node.id, "3:12");
        assert_eq!(node.text, "Save");
        assert!(node.clickable);
        assert_eq!(node.bounds, vec![10, 20, 110, 70]);
    }

    #[test]
    fn missing_fields_do_not_fail_the_parse() {
        // Forward compatibility in the direction that matters: an older service
        // on a container somebody has not reinstalled should degrade to fewer
        // properties, not to no tree at all.
        let node: BridgeNode = serde_json::from_str(r#"{"id":"1:0"}"#).unwrap();
        assert_eq!(node.id, "1:0");
        assert!(!node.clickable);
        assert!(node.bounds.is_empty());
    }

    #[test]
    fn a_tree_carries_its_generation() {
        let tree: Tree =
            serde_json::from_str(r#"{"generation":7,"nodes":[],"truncated":true}"#).unwrap();
        assert_eq!(tree.generation, 7);
        assert!(tree.truncated);
    }
}
