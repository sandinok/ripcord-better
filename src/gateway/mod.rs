//! Discord Gateway v10 client. zlib-stream + heartbeat + IDENTIFY/RESUME
//! + dispatch event queue.
//!
//! Source-of-truth for opcodes / lifecycle: research/gateway-spec.md.
//!
//! Architecture:
//!   - One `Gateway` task owns the WS stream.
//!   - On every inbound message we update `state` (Arc'd, shared with UI).
//!   - Outbound (heartbeats, IDENTIFY, RESUME, member requests) come in via
//!     a `mpsc::Sender`.
//!   - A separate `tokio::time::interval` fires heartbeat ticks; a missing
//!     ACK first gets a bounded grace window to arrive before the session
//!     is declared dead and resumed.

pub mod client;
pub mod events;
pub mod opcodes;
pub mod zlib;

use once_cell::sync::OnceCell;

pub use client::{Gateway, Outbound};

/// Process-wide outbound channel to the gateway task. The UI layer (member
/// list) uses it to send op 8 REQUEST_GUILD_MEMBERS without threading the
/// sender handle through every widget.
static OUTBOUND_TX: OnceCell<tokio::sync::mpsc::UnboundedSender<Outbound>> = OnceCell::new();

pub fn install_outbound(tx: tokio::sync::mpsc::UnboundedSender<Outbound>) -> Result<(), tokio::sync::mpsc::UnboundedSender<Outbound>> {
    OUTBOUND_TX.set(tx)
}

/// Ask the gateway for the full member list + presences of a guild (op 8).
/// Returns false when the gateway task is not running (e.g. signed out).
pub fn request_guild_members(guild_id: crate::model::Snowflake) -> bool {
    match OUTBOUND_TX.get() {
        Some(tx) => tx.send(Outbound::RequestMembers { guild_id }).is_ok(),
        None => false,
    }
}

/// True when a gateway task exists and its channel is still open.
pub fn gateway_alive() -> bool {
    OUTBOUND_TX.get().map(|tx| !tx.is_closed()).unwrap_or(false)
}
