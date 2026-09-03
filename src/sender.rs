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

use std::sync::Arc;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::model::Snowflake;
use crate::rest::endpoints::CreateMessageBody;
use crate::rest::Http;

/// (channel id, message body, nonce string for the optimistic echo).
pub type SendRequest = (Snowflake, CreateMessageBody, String);

/// Create the queue + spawn its single worker onto the current runtime.
/// The worker exits when every sender is dropped (app shutdown).
pub fn spawn_worker(rest: Arc<Http>, mut rx: UnboundedReceiver<SendRequest>) {
    tokio::spawn(async move {
        while let Some((cid, body, nonce)) = rx.recv().await {
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
                    let msg = match &e {
                        crate::rest::HttpError::RateLimited(_) => {
                            "Rate limited by Discord. Please wait a moment and press Enter again.".into()
                        }
                        crate::rest::HttpError::Discord(code, body) => {
                            format!("Discord rejected the message (HTTP {code}): {body}")
                        }
                        _ => format!("Could not deliver the message ({e})."),
                    };
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
}
