//! Message send queue.
//!
//! The composer NEVER fires REST calls directly. It prepares the message,
//! inserts the optimistic echo, and pushes one `SendRequest` onto this
//! queue. A single worker task drains it sequentially: one message at a
//! time, in order, with exactly one REST POST per request and no retry of
//! any kind.
//!
//! This is the structural half of the never-double-send guarantee: the UI
//! layer cannot race two POSTs for one Enter press because there is
//! exactly one queue and exactly one worker.
//!
//! v0.2 adds a local send governor (point 4 of the security spec):
//!   - a hard per-minute cap: extra sends are refused locally, surfaced
//!     to the user, never queued;
//!   - a hard stop on Discord "blocked / captcha / verification"
//!     responses: the queue keeps rejecting sends (with a persistent
//!     banner + a user-controlled Retry) until the user explicitly
//!     unlocks it. Basalt never unlocks itself.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::model::Snowflake;
use crate::rest::endpoints::CreateMessageBody;
use crate::rest::Http;

/// (channel id, message body, nonce string for the optimistic echo).
pub type SendRequest = (Snowflake, CreateMessageBody, String);

/// Local hard cap on outbound message POSTs per rolling minute. Discord's
/// own per-channel limit is 5 per 5 seconds and the global write limit
/// sits around 50 per minute; staying strictly below it locally means we
/// hit the API's limits only in genuine races, never by our own burst.
pub const MAX_SENDS_PER_MINUTE: usize = 30;

/// Sliding-window counter backing [`MAX_SENDS_PER_MINUTE`]. Global: the
/// JSON queue and the attachment upload path share one budget.
struct SendGovernor {
    timestamps: Mutex<Vec<Instant>>,
}

impl SendGovernor {
    const fn new() -> Self {
        Self {
            timestamps: Mutex::new(Vec::new()),
        }
    }

    /// Reserve one send slot. `Ok(())` = allowed; `Err(())` = capped.
    fn reserve(&self) -> Result<(), ()> {
        let now = Instant::now();
        let mut ts = self.timestamps.lock();
        ts.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        if ts.len() >= MAX_SENDS_PER_MINUTE {
            return Err(());
        }
        ts.push(now);
        Ok(())
    }
}

static GOVERNOR: once_cell::sync::Lazy<SendGovernor> =
    once_cell::sync::Lazy::new(SendGovernor::new);

/// Reserve one outbound send slot (shared by the queue worker and the
/// attachment upload path). `false` = local cap reached, do not send.
pub fn reserve_send_slot() -> bool {
    GOVERNOR.reserve().is_ok()
}

/// Does a Discord error response mean "sending is blocked" (captcha,
/// verification, spam-guard, account-level block)? Such a response must
/// halt ALL outbound sends until the user intervenes.
fn is_block_response(status: u16, body: &str) -> bool {
    if status == 403 {
        // 403 on message send = verification required / spam guard /
        // captcha wall, in every shape Discord uses.
        return true;
    }
    let low = body.to_ascii_lowercase();
    low.contains("captcha")
        || low.contains("you need to verify")
        || low.contains("account_verification_required")
        || low.contains("message sending is temporarily blocked")
        || low.contains("blocked from sending")
        || low.contains("spam")
}

/// Create the queue + spawn its single worker onto the current runtime.
/// The worker exits when every sender is dropped (app shutdown).
pub fn spawn_worker(rest: Arc<Http>, mut rx: UnboundedReceiver<SendRequest>) {
    tokio::spawn(async move {
        while let Some((cid, body, nonce)) = rx.recv().await {
            // A previous block locks the queue: fail fast, tell the user,
            // never auto-retry. Only state::clear_send_lock() (the Retry
            // button in the composer banner) reopens sending.
            if let Some(reason) = crate::state::global()
                .and_then(|s| s.send_lock_reason())
            {
                if let Some(s) = crate::state::global() {
                    s.fail_pending(
                        cid,
                        &nonce,
                        format!("Sending is paused: {reason}. Tap Retry after resolving it in Discord."),
                    );
                }
                continue;
            }
            // Local rate cap: refuse before Discord ever sees it.
            if !reserve_send_slot() {
                if let Some(s) = crate::state::global() {
                    s.fail_pending(
                        cid,
                        &nonce,
                        format!(
                            "Local rate cap reached ({MAX_SENDS_PER_MINUTE} messages per minute). Nothing was sent; try again shortly."
                        ),
                    );
                }
                continue;
            }
            // Exactly one POST per request. Never resent, never retried:
            // an ambiguous failure (timeout after Discord already stored
            // the message) must surface to the user, not silently resend.
            match rest.post_message(cid, &body).await {
                Ok(created) => {
                    if let Some(s) = crate::state::global() {
                        s.resolve_pending(cid, &nonce, created);
                    }
                }
                Err(e) => {
                    let mut block_reason: Option<String> = None;
                    let msg = match &e {
                        crate::rest::HttpError::RateLimited(_) => {
                            "Rate limited by Discord. Please wait a moment and press Enter again.".into()
                        }
                        crate::rest::HttpError::Discord(code, body) => {
                            if is_block_response(code.as_u16(), body) {
                                block_reason = Some(
                                    "Discord blocked message sending (verification, captcha or spam guard)".to_string(),
                                );
                                "Discord blocked sending. Basalt stopped all sends for your safety.".to_string()
                            } else {
                                format!("Discord rejected the message (HTTP {code}): {body}")
                            }
                        }
                        _ => format!("Could not deliver the message ({e})."),
                    };
                    if let Some(reason) = block_reason {
                        if let Some(s) = crate::state::global() {
                            s.lock_sends(reason);
                        }
                    }
                    if let Some(s) = crate::state::global() {
                        s.fail_pending(cid, &nonce, msg);
                    }
                }
            }
        }
    });
}

/// Convenience: build the queue pair.
pub fn channel() -> (UnboundedSender<SendRequest>, UnboundedReceiver<SendRequest>) {
    mpsc::unbounded_channel()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_preserves_order_and_count() {
        // The queue is the serialization point for sends; mpsc guarantees
        // FIFO and exactly-once delivery per send() call.
        let (tx, mut rx) = channel();
        for i in 0..3 {
            let _ = tx.send((Snowflake(i), CreateMessageBody::default(), i.to_string()));
        }
        drop(tx);
        let mut got = Vec::new();
        while let Some((cid, _, nonce)) = rx.blocking_recv() {
            got.push((cid.0, nonce));
        }
        assert_eq!(
            got,
            vec![
                (0u64, "0".to_string()),
                (1u64, "1".to_string()),
                (2u64, "2".to_string())
            ]
        );
    }

    #[test]
    fn governor_caps_at_limit() {
        use std::sync::Mutex as StdMutex;
        static ONCE: StdMutex<()> = StdMutex::new(());
        let _guard = ONCE.lock().unwrap_or_else(|e| e.into_inner());
        let g = &*GOVERNOR;
        for _ in 0..MAX_SENDS_PER_MINUTE {
            assert!(g.reserve().is_ok(), "first MAX sends must pass");
        }
        assert!(g.reserve().is_err(), "send {MAX_SENDS_PER_MINUTE:+} must be refused");
    }

    #[test]
    fn block_detector_catches_the_real_markers() {
        assert!(is_block_response(403, "{\"code\": 0}"));
        assert!(is_block_response(400, "{\"message\": \"captcha required\"}"));
        assert!(is_block_response(
            400,
            "{\"message\": \"You need to verify your account\"}"
        ));
        assert!(is_block_response(400, "{\"code\": 40004, \"message\": \"spam\"}"));
        // Ordinary errors must NOT lock sending.
        assert!(!is_block_response(400, "{\"code\": 50035, \"message\": \"Invalid Form Body\"}"));
        assert!(!is_block_response(404, "{\"message\": \"Unknown Channel\"}"));
    }
}
