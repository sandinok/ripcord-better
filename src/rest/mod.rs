//! Discord REST v10 client. Endpoints + rate-limit handling + multipart upload.
//!
//! Source-of-truth for endpoint paths: research/rest-spec.md.
//!
//! Rate-limit strategy: per-bucket `Mutex<TokenBucket>` keyed by the
//! `X-RateLimit-Bucket` header. 429 responses with `Retry-After` are
//! retried with exponential backoff (max 3 attempts).

pub mod client;
pub mod endpoints;
pub mod ratelimit;

use std::sync::Arc;

use once_cell::sync::OnceCell;

pub use client::{guess_mime, Http, HttpError};

/// Process-wide handle to the shared Http client, registered by the app at
/// startup so UI modules (members panel, resolvers) can fire requests
/// without plumbing the Arc through every render call.
static GLOBAL_HTTP: OnceCell<Arc<Http>> = OnceCell::new();

pub fn install_global(http: Arc<Http>) -> Result<(), Arc<Http>> {
    GLOBAL_HTTP.set(http)
}

pub fn global() -> Option<Arc<Http>> {
    GLOBAL_HTTP.get().cloned()
}

/// A bare JSON-capable HTTP client for non-Discord APIs (YouTube oEmbed,
/// release checks). No auth, no rate limiting.
pub fn plain_client() -> reqwest::Client {
    static PLAIN: once_cell::sync::Lazy<reqwest::Client> = once_cell::sync::Lazy::new(|| {
        reqwest::Client::builder()
            .user_agent(crate::identity::image_user_agent())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("plain http client init")
    });
    PLAIN.clone()
}
