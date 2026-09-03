//! Discord Gateway v10 client. zlib-stream + heartbeat + IDENTIFY/RESUME
//! + dispatch event queue.
//!
//! Source-of-truth for opcodes / lifecycle: research/gateway-spec.md.
//!
//! Architecture:
//!   - One `Gateway` task owns the WS stream.
//!   - On every inbound message we update `state` (Arc'd, shared with UI).
//!   - Outbound (heartbeats, IDENTIFY, RESUME, message sends) come in via
//!     a `mpsc::Sender`.
//!   - A separate `tokio::time::interval` fires heartbeat ticks; if the
//!     prior heartbeat's ACK hasn't landed within `heartbeat_interval`
//!     seconds × 1.5, we close and reconnect with RESUME.

pub mod client;
pub mod events;
pub mod opcodes;
pub mod zlib;

pub use client::{Gateway, Outbound};
