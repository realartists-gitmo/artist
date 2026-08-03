//! Reconnectable local host boundary for one live Artist root session.

pub mod core;
#[cfg(target_os = "linux")]
pub mod daemon;
pub mod protocol;
pub mod registry;
pub mod replay;
#[cfg(target_os = "linux")]
pub mod transport;

pub use core::{HostCore, HostReply, RuntimeAction};
#[cfg(target_os = "linux")]
pub use daemon::{DaemonOptions, SessionHostDaemon};
pub use protocol::{
    AttentionKind, HostCommand, HostEvent, HostRequest, HostResponse, PROTOCOL_VERSION,
    RuntimePhase, RuntimeState, SequencedHostEvent, ServerPacket, StageDescriptor,
};
pub use registry::{HostLease, HostRecord, HostRegistry};
pub use replay::{EventJournal, Sequenced};
#[cfg(target_os = "linux")]
pub use transport::{PeerCredentials, SeqPacket, SeqPacketListener};
