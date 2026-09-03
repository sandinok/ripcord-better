//! HTTP client. Bearer-or-bot authorization, Rustls TLS, JSON bodies,
//! per-route rate-limit buckets. Wraps `reqwest`.

use std::time::Duration;

use anyhow::Result;
use parking_lot::RwLock;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::{Client, Method, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::model::Snowflake;
use super::endpoints::Route;
use super::ratelimit::RateLimiter;

const API_BASE: &str = "https://discord.com/api/v10";

/// Discord REST errors. Distinct because most callers want to
/// distinguish 4xx (don't retry) from 5xx (retry with backoff) from
/// network failures (retry forever).
#[derive(Error, Debug)]
pub enum HttpError {
    #[error("discord rest: status {0} body: {1}")]
    Discord(StatusCode, String),
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),
    #[error("rate-limited by Discord (bucket={0}). The request was NOT resent.")]
    RateLimited(String),
    #[error("decode json: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("not authorized: no token configured")]
    NoToken,
    #[error("build request: {0}")]
    Build(reqwest::Error),
}

pub struct Http {
    client: Client,
    token: RwLock<Option<String>>,
    /// When true, the Authorization header uses the `Bot <token>` prefix
    /// (bot accounts). Auto-detected on the first 401.
    bot_prefix: RwLock<bool>,
    limiter: RateLimiter,
    /// Track the `X-RateLimit-Global` flag centrally so a global rate-limit
    /// blocks *all* requests across all routes for the Retry-After duration.
    /// `Pin<Box<Sleep>>` because `tokio::time::Sleep` is `!Unpin` (contains
    /// `PhantomPinned`), so we can't store a bare `Sleep` and `.await` it later.
    global_pause: Mutex<Option<std::pin::Pin<Box<tokio::time::Sleep>>>>,
}

impl Http {
    pub fn new(token: Option<String>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        // A User-Agent is REQUIRED on every request — Cloudflare 403s the
        // default reqwest UA. The actual UA is chosen per request (see
        // `ua_for_request`) so the REST identity always matches the gateway
        // IDENTIFY properties: web-client UA for user sessions, the
        // documented bot format once the token turns out to be a bot.
        headers.insert(USER_AGENT, HeaderValue::from_static(crate::identity::PLACEHOLDER_UA));
        headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("application/json"),
        );
        // Always send Accept-Encoding: gzip — reqwest will transparently decode
        // when the `gzip` feature is enabled (which it is in Cargo.toml).
        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(8)
            .tcp_nodelay(true)
            .https_only(true)
            .gzip(true)
            .build()
            .map_err(|e| anyhow::anyhow!("reqwest builder: {e}"))?;
        Ok(Self {
            client,
            token: RwLock::new(token),
            bot_prefix: RwLock::new(false),
            limiter: RateLimiter::new(),
            global_pause: Mutex::new(None),
        })
    }

    pub fn set_token(&self, token: String) {
        *self.token.write() = Some(token);
        *self.bot_prefix.write() = false;
    }
    pub fn clear_token(&self) {
        *self.token.write() = None;
    }
    /// Use the `Bot <token>` Authorization prefix (for bot accounts).
    pub fn set_bot_prefix(&self, on: bool) {
        *self.bot_prefix.write() = on;
    }
    pub fn uses_bot_prefix(&self) -> bool {
        *self.bot_prefix.read()
    }

    // ── Typed endpoints ──

    pub async fn get_current_user(&self) -> Result<crate::model::User, HttpError> {
        self.get_json(Route::CurrentUser).await
    }
    pub async fn get_my_guilds(&self, with_counts: bool) -> Result<Vec<crate::model::Guild>, HttpError> {
        let path = if with_counts {
            "/users/@me/guilds?with_counts=true"
        } else {
            "/users/@me/guilds"
        };
        // We re-use the Route::MyGuilds bucket for both variants — Discord
        // treats them as the same bucket.
        self.get_json_with_path(Route::MyGuilds, path).await
    }
    pub async fn get_guild_channels(&self, guild_id: Snowflake) -> Result<Vec<crate::model::Channel>, HttpError> {
        let path = format!("/guilds/{}/channels", guild_id);
        self.get_json_with_path(Route::GuildChannels(guild_id), &path).await
    }
    pub async fn get_my_dm_channels(&self) -> Result<Vec<crate::model::Channel>, HttpError> {
        // NOTE: bot accounts get their DM list here; user accounts too.
        self.get_json_with_path(Route::MyGuilds, "/users/@me/channels").await
    }
    pub async fn get_user(&self, user_id: Snowflake) -> Result<crate::model::User, HttpError> {
        self.get_json(Route::User(user_id)).await
    }
    /// NOTE: requires the GUILD_MEMBERS privileged intent for bots.
    pub async fn list_guild_members(
        &self,
        guild_id: Snowflake,
        limit: u8,
    ) -> Result<Vec<crate::model::Member>, HttpError> {
        let limit = (limit as u16).clamp(1, 1000) as u8;
        let path = format!("/guilds/{}/members?limit={}", guild_id, limit);
        self.get_json_with_path(Route::Guild(guild_id), &path).await
    }
    pub async fn get_channel_messages(
        &self,
        channel_id: Snowflake,
        limit: u8,
        before: Option<Snowflake>,
    ) -> Result<Vec<crate::model::Message>, HttpError> {
        let limit = limit.clamp(1, 100);
        let mut path = format!("/channels/{}/messages?limit={}", channel_id, limit);
        if let Some(b) = before {
            path.push_str(&format!("&before={}", b.0));
        }
        // Discord treats each channel as a separate bucket, so we don't
        // include the query string in the bucket key.
        let mut msgs: Vec<crate::model::Message> =
            self.get_json_with_path(Route::ChannelMessages(channel_id), &path).await?;
        // The API returns newest-first; the UI renders chronologically.
        msgs.reverse();
        Ok(msgs)
    }
    pub async fn get_channel(&self, channel_id: Snowflake) -> Result<crate::model::Channel, HttpError> {
        let path = format!("/channels/{}", channel_id);
        self.get_json_with_path(Route::Channel(channel_id), &path).await
    }
    pub async fn post_message(
        &self,
        channel_id: Snowflake,
        body: &crate::rest::endpoints::CreateMessageBody,
    ) -> Result<crate::model::Message, HttpError> {
        let path = format!("/channels/{}/messages", channel_id);
        self.post_json(Route::ChannelMessages(channel_id), &path, body).await
    }
    pub async fn trigger_typing(&self, channel_id: Snowflake) -> Result<(), HttpError> {
        let path = format!("/channels/{}/typing", channel_id);
        let _: serde_json::Value = self
            .post_json(Route::ChannelTyping(channel_id), &path, &serde_json::json!({}))
            .await?;
        Ok(())
    }
    pub async fn create_reaction(&self, channel_id: Snowflake, message_id: Snowflake, emoji: &str) -> Result<(), HttpError> {
        // PUT, not POST — see research/rest-spec.md §3 (Create Reaction correction).
        let path = format!(
            "/channels/{}/messages/{}/reactions/{}/@me",
            channel_id, message_id, emoji
        );
        let _: serde_json::Value = self
            .request_empty(Route::ChannelReactions(channel_id), Method::PUT, &path)
            .await?;
        Ok(())
    }

    /// Pinned messages of a channel, newest first (Discord's order).
    pub async fn get_channel_pins(&self, channel_id: Snowflake) -> Result<Vec<crate::model::Message>, HttpError> {
        let path = format!("/channels/{}/pins", channel_id);
        self.get_json_with_path(Route::ChannelPins(channel_id), &path).await
    }

    // ── Low-level request helpers ──

    async fn get_json<T: DeserializeOwned>(&self, route: Route) -> Result<T, HttpError> {
        let path = route.path();
        self.get_json_with_path(route, &path).await
    }

    async fn get_json_with_path<T: DeserializeOwned>(
        &self,
        route: Route,
        path: &str,
    ) -> Result<T, HttpError> {
        let url = format!("{}{}", API_BASE, path);
        self.acquire_attempt(&route).await?;
        // Snapshot the token into an owned `Option<String>` so the
        // `RwLockReadGuard` is dropped *before* the `.send().await`. The guard
        // contains a `*mut ()` (parking_lot), so holding it across an await
        // would make this future `!Send`, which would break every
        // `tokio::spawn` / `Handle::spawn` call site in the UI layer.
        let token_owned = self.token.read().clone();
        let bot_prefix = *self.bot_prefix.read();
        let resp = self.client
            .request(Method::GET, &url)
            .send_opt_auth(token_owned.as_deref(), bot_prefix)
            .send()
            .await
            .map_err(HttpError::Network)?;
        let resp = self.handle_response(route, resp).await?;
        resp.json::<T>().await.map_err(HttpError::Network)
    }

    async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        route: Route,
        path: &str,
        body: &B,
    ) -> Result<T, HttpError> {
        let url = format!("{}{}", API_BASE, path);
        self.acquire_attempt(&route).await?;
        let body = serde_json::to_vec(body).map_err(HttpError::Decode)?;
        // See `get_json_with_path` for why we snapshot the token here.
        let token_owned = self.token.read().clone();
        let bot_prefix = *self.bot_prefix.read();
        let resp = self.client
            .request(Method::POST, &url)
            .header(CONTENT_TYPE, "application/json")
            .send_opt_auth(token_owned.as_deref(), bot_prefix)
            .body(body)
            .send()
            .await
            .map_err(HttpError::Network)?;
        let resp = self.handle_response(route, resp).await?;
        resp.json::<T>().await.map_err(HttpError::Network)
    }

    /// Sends an empty-body request (used for PUTs that take no body like create-reaction).
    async fn request_empty(
        &self,
        route: Route,
        method: Method,
        path: &str,
    ) -> Result<serde_json::Value, HttpError> {
        let url = format!("{}{}", API_BASE, path);
        self.acquire_attempt(&route).await?;
        // See `get_json_with_path` for why we snapshot the token here.
        let token_owned = self.token.read().clone();
        let bot_prefix = *self.bot_prefix.read();
        let resp = self.client
            .request(method, &url)
            .header(CONTENT_TYPE, "application/json")
            .send_opt_auth(token_owned.as_deref(), bot_prefix)
            .send()
            .await
            .map_err(HttpError::Network)?;
        let resp = self.handle_response(route, resp).await?;
        // Empty bodies come back as 204 No Content — return an empty JSON object.
        let txt = resp.text().await.map_err(HttpError::Network)?;
        if txt.is_empty() {
            Ok(serde_json::json!({}))
        } else {
            serde_json::from_str(&txt).map_err(HttpError::Decode)
        }
    }

    async fn acquire_attempt(&self, route: &Route) -> Result<(), HttpError> {
        // Wait on global pause if active.
        loop {
            let mut gp = self.global_pause.lock().await;
            if let Some(sleep) = gp.as_mut() {
                sleep.await;
                *gp = None;
            } else {
                break;
            }
        }
        self.limiter.acquire(route).await
    }

    async fn handle_response(&self, route: Route, resp: Response) -> Result<Response, HttpError> {
        let status = resp.status();
        let bucket = resp
            .headers()
            .get("X-RateLimit-Bucket")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if let Some(b) = &bucket {
            // Update remaining / reset-after for this bucket.
            self.limiter.update_from_headers(&route, b.clone(), resp.headers());
        }
        // 429 → wait the full Retry-After (strictly, per Discord's docs),
        // then report the failure to the caller. Requests are NEVER resent
        // automatically: a resend after an ambiguous failure is exactly how
        // duplicate messages and spam strikes happen. The error carries the
        // bucket; the UI shows it to the user.
        if status == StatusCode::TOO_MANY_REQUESTS {
            // Read headers before `text()` consumes the response.
            let (retry_after_ms, is_global) = {
                let hdrs = resp.headers().clone();
                let body = resp.text().await.unwrap_or_default();
                parse_429(&body, &hdrs)
            };
            tracing::warn!(
                bucket = ?bucket, retry_ms = retry_after_ms, global = is_global,
                "discord 429"
            );
            if is_global {
                // A global rate-limit blocks *all* routes: park a shared
                // sleep so in-flight and future requests wait it out.
                let dur = Duration::from_millis(retry_after_ms + 250);
                let mut gp = self.global_pause.lock().await;
                *gp = Some(Box::pin(tokio::time::sleep(dur)));
            } else {
                // Bucket-level: mark the bucket exhausted for the full
                // window so queued requests wait instead of piling 429s,
                // then sleep this caller out too.
                self.limiter.mark_exhausted(&route, Duration::from_millis(retry_after_ms));
                tokio::time::sleep(Duration::from_millis(retry_after_ms + 50)).await;
            }
            return Err(HttpError::RateLimited(bucket.unwrap_or_default()));
        }
        if status.is_success() {
            return Ok(resp);
        }
        // Other error codes — surface body to caller.
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(status = %status, route = ?route, body = %body, "discord rest error");
        Err(HttpError::Discord(status, body))
    }
}

/// Parse a 429 response. Returns (retry_after_ms, is_global).
///
/// Discord puts the authoritative `retry_after` (seconds, float) in the JSON
/// body; the `Retry-After` header is the HTTP-standard fallback (seconds).
/// `X-RateLimit-Global: true` marks a Cloudflare-level global limit.
fn parse_429(body: &str, headers: &HeaderMap) -> (u64, bool) {
    let mut retry_after_ms: Option<u64> = None;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(secs) = v.get("retry_after").and_then(|r| r.as_f64()) {
            retry_after_ms = Some((secs * 1000.0).ceil() as u64);
        }
    }
    if retry_after_ms.is_none() {
        if let Some(v) = headers.get("Retry-After").and_then(|v| v.to_str().ok()) {
            if let Ok(secs) = v.parse::<f64>() {
                retry_after_ms = Some((secs * 1000.0).ceil() as u64);
            }
        }
    }
    let is_global = headers
        .get("X-RateLimit-Global")
        .and_then(|v| v.to_str().ok())
        .map(|s| s == "true")
        .unwrap_or(false);
    (retry_after_ms.unwrap_or(1000), is_global)
}

// ── Helper trait for adding the Authorization header only when we have a token.
trait RequestBuilderExt {
    fn send_opt_auth(self, token: Option<&str>, bot_prefix: bool) -> Self;
}

impl RequestBuilderExt for reqwest::RequestBuilder {
    fn send_opt_auth(mut self, token: Option<&str>, bot_prefix: bool) -> Self {
        // UA first: user sessions keep the web-client UA; bot sessions use
        // the documented bot format. Per-request headers override defaults.
        let ua = if bot_prefix {
            crate::identity::bot_user_agent()
        } else {
            crate::identity::web_user_agent()
        };
        if let Ok(v) = HeaderValue::from_str(&ua) {
            self = self.header(USER_AGENT, v);
        }
        if let Some(t) = token {
            if !t.is_empty() {
                let value = if bot_prefix { format!("Bot {t}") } else { t.to_string() };
                let v = HeaderValue::from_str(&value).expect("token contains invalid chars");
                self = self.header(AUTHORIZATION, v);
            }
        }
        self
    }
}
