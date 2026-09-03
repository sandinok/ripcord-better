//! Per-bucket rate limiting. Discord's rate-limit headers:
//!   - X-RateLimit-Limit       (max requests per window per bucket)
//!   - X-RateLimit-Remaining   (remaining in current window)
//!   - X-RateLimit-Reset       (epoch seconds when window resets)
//!   - X-RateLimit-Reset-After (seconds until reset, fractional)
//!   - X-RateLimit-Bucket      (bucket UUID — same across similar routes)
//!   - X-RateLimit-Global       (true when the 429 is global, not bucket-scoped)
//!   - Retry-After              (seconds; on 429 only)
//!
//! Our strategy: minimal state. We don't pre-emptively throttle per-bucket
//! (most routes are 5/req/s — we don't come close). We only:
//!   1. Track a `tokio::time::Sleep` per bucket that fires when the reset
//!      would have completed. If a request would land *before* the reset
//!      sleep is done AND we have 0 remaining, we delay until the sleep
//!      resolves. (Otherwise we proceed.)
//!   2. Honor the `Retry-After` from 429s via the client's backoff loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use reqwest::header::HeaderMap;

use super::endpoints::Route;

#[derive(Default)]
struct BucketState {
    /// Bucket UUID (from X-RateLimit-Bucket). None until first response.
    bucket_id: Option<String>,
    /// Remaining tokens in current window.
    remaining: i64,
    /// Reset time relative to local monotonic clock.
    reset_at: Option<Instant>,
    /// Limit per window (informational).
    limit: Option<u64>,
}

pub struct RateLimiter {
    buckets: Mutex<HashMap<Route, Arc<Mutex<BucketState>>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self { buckets: Mutex::new(HashMap::new()) }
    }

    /// Returns a fresh attempt counter for the request. If the route's
    /// bucket has 0 remaining tokens and the reset is in the future, this
    /// function sleeps until reset+1ms.
    pub async fn acquire(&self, route: &Route) -> Result<(), super::HttpError> {
        let bucket = {
            let mut g = self.buckets.lock();
            g.entry(route.clone())
                .or_insert_with(|| Arc::new(Mutex::new(BucketState::default())))
                .clone()
        };
        // Check under the per-bucket lock whether we need to wait.
        // We deliberately take this lock briefly (no .await inside).
        let wait: Option<Duration> = {
            let st = bucket.lock();
            if st.remaining == 0 {
                if let Some(reset) = st.reset_at {
                    let now = Instant::now();
                    if reset > now {
                        Some(reset - now + Duration::from_millis(5))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(d) = wait {
            tokio::time::sleep(d).await;
        }
        Ok(())
    }

    pub fn update_from_headers(&self, route: &Route, bucket_id: String, headers: &HeaderMap) {
        let bucket = {
            let mut g = self.buckets.lock();
            g.entry(route.clone())
                .or_insert_with(|| Arc::new(Mutex::new(BucketState::default())))
                .clone()
        };
        let mut st = bucket.lock();
        st.bucket_id = Some(bucket_id);
        if let Some(v) = headers.get("X-RateLimit-Limit").and_then(|v| v.to_str().ok()) {
            if let Ok(n) = v.parse::<u64>() {
                st.limit = Some(n);
            }
        }
        if let Some(v) = headers.get("X-RateLimit-Remaining").and_then(|v| v.to_str().ok()) {
            if let Ok(n) = v.parse::<i64>() {
                st.remaining = n;
            }
        }
        if let Some(v) = headers.get("X-RateLimit-Reset-After").and_then(|v| v.to_str().ok()) {
            if let Ok(secs) = v.parse::<f64>() {
                st.reset_at = Some(Instant::now() + Duration::from_secs_f64(secs));
            }
        }
    }

    /// Mark a bucket as exhausted for the next `window` (used when Discord
    /// answers 429 so queued requests on the same route wait it out instead
    /// of stacking more 429s).
    pub fn mark_exhausted(&self, route: &Route, window: Duration) {
        let bucket = {
            let mut g = self.buckets.lock();
            g.entry(route.clone())
                .or_insert_with(|| Arc::new(Mutex::new(BucketState::default())))
                .clone()
        };
        let mut st = bucket.lock();
        st.remaining = 0;
        st.reset_at = Some(Instant::now() + window + Duration::from_millis(50));
    }
}
