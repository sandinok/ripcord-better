//! Gateway client. Owns the WS connection, the heartbeat timer, the
//! sequence counter, and the inbound/outbound mpsc channels that the
//! rest of the app uses to talk to it.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::state::AppState;
use super::events::parse_dispatch;
use super::opcodes::intents;
use super::opcodes::{GatewayOp, CLIENT_CAPABILITIES};
use super::zlib::GatewayZlib;

/// WebSocket stream type alias (split sink half).
type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMessage,
>;
/// WebSocket stream type alias (split stream half).
type WsStreamHalf = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream";

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("ws: {0}")]
    Ws(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not authenticated: no token set")]
    NoToken,
    #[error("bad hello: missing heartbeat_interval")]
    BadHello,
    #[error("authentication failed (close code 4004)")]
    AuthFailed,
    #[error("disallowed intent (close code 4014) — drop privileged intents")]
    DisallowedIntent,
    #[error("shutdown requested")]
    Shutdown,
    #[error("reconnect exhausted: {0}")]
    ReconnectExhausted(String),
    #[error("other: {0}")]
    Other(#[from] anyhow::Error),
}

/// What the UI / main app sends to the gateway task.
pub enum Outbound {
    /// Open the WS connection with this token. `bot` selects the
    /// bot-style IDENTIFY (user-client fields break bot sessions).
    Connect { token: String, bot: bool },
    /// Send a presence update (the gateway accepts this any time after READY).
    SetPresence { status: String, afk: bool },
    /// Ask Discord for the full member list of a guild (op 8). Delivered
    /// back as GUILD_MEMBERS_CHUNK dispatches, which feed the member list.
    RequestMembers { guild_id: crate::model::Snowflake },
    /// User-authorized manual reconnect (the Retry button on the
    /// "connection lost" banner after the automatic-attempt cap).
    Reconnect,
    /// Soft-shutdown the gateway task.
    Shutdown,
}

/// Build the op-8 REQUEST_GUILD_MEMBERS payload: `query: ""` + `limit: 0`
/// means "the entire guild, in chunks"; `presences: true` asks Discord to
/// attach status dots to every member it sends.
fn request_members_payload(guild_id: crate::model::Snowflake) -> serde_json::Value {
    serde_json::json!({
        "op": GatewayOp::RequestGuildMembers as u8,
        "d": {
            "guild_id": guild_id.0.to_string(),
            "query": "",
            "limit": 0,
            "presences": true,
        }
    })
}

pub struct Gateway {
    state: Arc<AppState>,
    inbound: mpsc::UnboundedSender<GatewayOp>, // mirror out for connection control
    outbound_rx: Mutex<Option<mpsc::UnboundedReceiver<Outbound>>>,
    outbound_tx: mpsc::UnboundedSender<Outbound>,
}

impl Gateway {
    pub fn new(state: Arc<AppState>) -> Self {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        // inbound not currently used; placeholder for outbound op 1/etc.
        let (inbound, _inbound_rx) = mpsc::unbounded_channel();
        Self {
            state,
            inbound,
            outbound_rx: Mutex::new(Some(outbound_rx)),
            outbound_tx,
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Outbound> {
        self.outbound_tx.clone()
    }

    pub fn spawn(self: Arc<Self>) {
        // Publish the outbound sender process-wide so UI code (member list)
        // can request guild members via op 8 without threading the handle
        // through every widget.
        let _ = super::install_outbound(self.outbound_tx.clone());
        tokio::spawn(async move {
            let state = self.state.clone();
            let mut outbound_rx = self.outbound_rx.lock().take().expect("outbound_rx taken twice");
            let mut token: Option<String> = None;
            let mut is_bot = false;
            let mut shutdown = false;
            // Reconnect backoff state: exponential with jitter, reset after
            // a connection survives long enough. See sleep_backoff.
            let mut backoff: u64 = BASE_BACKOFF_MS;
            // Consecutive failed connections. A stable connection resets
            // it to 0. When it reaches MAX_RECONNECT_ATTEMPTS the loop
            // stops reconnecting (zero reconnect storms) and waits for a
            // user-authorized Outbound::Reconnect.
            let mut failed_attempts: u32 = 0;
            let mut halted = false;
            // Start with the full intent set (members, presences, message
            // content); if Discord closes with 4014 we retry once with the
            // baseline set so the app still works, just without presence.
            let mut intent_set = intents::FULL;

            while !shutdown {
                if halted {
                    // Hard stop: park until the user hits Retry, signs in
                    // again, or shuts down. No reconnect storm, ever.
                    loop {
                        tokio::select! {
                            biased;
                            Some(cmd) = outbound_rx.recv() => {
                                match cmd {
                                    Outbound::Connect { token: t, bot } => {
                                        if !t.is_empty() {
                                            token = Some(t);
                                            is_bot = bot;
                                        }
                                        halted = false;
                                        failed_attempts = 0;
                                        backoff = BASE_BACKOFF_MS;
                                        intent_set = intents::FULL;
                                        state.set_intents_limited(false);
                                        break;
                                    }
                                    Outbound::Reconnect => {
                                        halted = false;
                                        failed_attempts = 0;
                                        backoff = BASE_BACKOFF_MS;
                                        break;
                                    }
                                    Outbound::Shutdown => { shutdown = true; break; }
                                    _ => {}
                                }
                            }
                        }
                        if shutdown {
                            break;
                        }
                    }
                    if shutdown {
                        break;
                    }
                }
                // Wait for the FIRST connect command only. After that the
                // loop is self-driving: a dropped connection reconnects on
                // its own (with backoff), instead of parking until some UI
                // widget happens to send a command. (The old "block on
                // recv() between connections" shape was the root cause of
                // the sticky "Connection lost" banner when the member
                // panel was hidden or a DM was open.)
                while token.is_none() && !shutdown {
                    tokio::select! {
                        biased;
                        Some(cmd) = outbound_rx.recv() => {
                            match cmd {
                                Outbound::Connect { token: t, bot } => {
                                    if t.is_empty() {
                                        tracing::warn!("ignoring empty-token connect");
                                        continue;
                                    }
                                    token = Some(t);
                                    is_bot = bot;
                                    // A fresh connect resets the intent attempt.
                                    intent_set = intents::FULL;
                                    state.set_intents_limited(false);
                                }
                                Outbound::SetPresence { .. } => {
                                    // Not connected yet: the status will be sent
                                    // with the IDENTIFY payload on the next connect.
                                }
                                Outbound::RequestMembers { .. } => {
                                    // Not connected yet: the member panel retries
                                    // via its REST fallback; re-request later.
                                }
                                Outbound::Reconnect => {
                                    // No session yet; nothing to reconnect.
                                }
                                Outbound::Shutdown => { shutdown = true; break; }
                            }
                        }
                    }
                }
                if shutdown {
                    break;
                }
                let Some(token) = token.clone() else { continue; };

                // Run the connection. Returns a status code telling us what to do next.
                let started = std::time::Instant::now();
                let outcome = run_connection_loop(
                    token.clone(),
                    is_bot,
                    self.state.clone(),
                    self.outbound_tx.clone(),
                    &mut outbound_rx,
                    intent_set,
                ).await;
                match outcome {
                    Ok(ConnOutcome::CleanShutdown) => { shutdown = true; }
                    Ok(ConnOutcome::ReconnectFresh) => {
                        tracing::info!("gateway reconnecting fresh (IDENTIFY)");
                        state.clear_session().await;
                        if started.elapsed() < STABLE_CONN_SECS {
                            failed_attempts += 1;
                            backoff = backoff.saturating_mul(2).min(MAX_BACKOFF_MS);
                        } else {
                            failed_attempts = 0;
                            backoff = BASE_BACKOFF_MS;
                        }
                    }
                    Ok(ConnOutcome::ReconnectResume) => {
                        tracing::info!("gateway reconnecting (RESUME)");
                        // RESUME right after a drop is expected behavior; only
                        // repeated fast churn backs off.
                        if started.elapsed() < STABLE_CONN_SECS {
                            failed_attempts += 1;
                            backoff = backoff.max(BASE_BACKOFF_MS).saturating_mul(2).min(MAX_BACKOFF_MS);
                        } else {
                            failed_attempts = 0;
                            backoff = BASE_BACKOFF_MS;
                        }
                    }
                    Err(GatewayError::AuthFailed) => {
                        tracing::error!("auth failed (close code 4004) - token is bad; halting");
                        state.set_connection_status(crate::state::ConnectionStatus::AuthFailed).await;
                        break;
                    }
                    Err(GatewayError::DisallowedIntent) => {
                        if intent_set == intents::FULL {
                            tracing::warn!(
                                "disallowed intent (close 4014) - retrying with baseline intents");
                            intent_set = intents::BASELINE;
                            state.set_intents_limited(true);
                            state.clear_session().await;
                            // Our own one-shot retry, not a server flap: the
                            // backoff ladder is untouched.
                        } else {
                            tracing::error!("disallowed intent even with baseline set - halting");
                            state.set_connection_status(
                                crate::state::ConnectionStatus::DisallowedIntent,
                            ).await;
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "connection loop ended; backing off");
                        failed_attempts += 1;
                        backoff = if started.elapsed() < STABLE_CONN_SECS {
                            backoff.max(BASE_BACKOFF_MS).saturating_mul(2).min(MAX_BACKOFF_MS)
                        } else {
                            BASE_BACKOFF_MS
                        };
                    }
                }
                // The reconnect-attempt cap: after MAX_RECONNECT_ATTEMPTS
                // consecutive unstable connections we park and wait for a
                // user-initiated retry (point 5: zero reconnect storms).
                if failed_attempts >= MAX_RECONNECT_ATTEMPTS {
                    tracing::error!(
                        attempts = failed_attempts,
                        "reconnect cap reached - halting until user retries"
                    );
                    state
                        .set_connection_status(crate::state::ConnectionStatus::Disconnected)
                        .await;
                    halted = true;
                    continue;
                }
                if !halted {
                    sleep_backoff(backoff).await;
                }
            }
            tracing::info!("gateway task exiting");
        });
    }
}

/// Base reconnect backoff, milliseconds. Doubles per consecutive failure,
/// capped at MAX_BACKOFF_MS, reset once a connection survives
/// STABLE_CONN_SECS.
const BASE_BACKOFF_MS: u64 = 1_000;
const MAX_BACKOFF_MS: u64 = 60_000;

/// Consecutive unstable connections before we park and wait for a
/// user-authorized retry. Discord's own client gives up similarly; this
/// prevents reconnect storms from bad networks or expired sessions.
const MAX_RECONNECT_ATTEMPTS: u32 = 20;
const STABLE_CONN_SECS: Duration = Duration::from_secs(30);

/// Sleep `ms` with full jitter (uniform random in [0.6*ms, 1.3*ms]): clients
/// that fail together (network blip) must not reconnect as a synchronized
/// thundering herd.
async fn sleep_backoff(ms: u64) {
    if ms == 0 {
        return;
    }
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    let rand01 = (u64::from_le_bytes(b) % 1_000_000) as f64 / 1_000_000.0;
    let jittered = (ms as f64 * 0.6) + rand01 * (ms as f64 * 0.7);
    tokio::time::sleep(Duration::from_millis(jittered.max(50.0) as u64)).await;
}

#[cfg(test)]
mod tests {
    #[test]
    fn backoff_ladder_doubles_and_caps() {
        let mut b = super::BASE_BACKOFF_MS;
        let mut steps = Vec::new();
        for _ in 0..10 {
            steps.push(b);
            b = b.saturating_mul(2).min(super::MAX_BACKOFF_MS);
        }
        assert_eq!(steps[0], 1_000);
        assert_eq!(steps[1], 2_000);
        assert_eq!(steps[5], 32_000);
        assert!(steps.iter().all(|&s| s <= super::MAX_BACKOFF_MS));
        assert_eq!(steps[9], super::MAX_BACKOFF_MS);
    }

    /// op 8 payload shape: query="" + limit=0 means "the whole guild", and
    /// guild_id must serialize as a STRING (the wire format Discord expects).
    #[test]
    fn request_members_payload_shape() {
        let v = super::request_members_payload(crate::model::Snowflake::from_u64(123));
        assert_eq!(v["op"], 8);
        assert_eq!(v["d"]["guild_id"], serde_json::json!("123"));
        assert_eq!(v["d"]["query"], "");
        assert_eq!(v["d"]["limit"], 0);
        assert_eq!(v["d"]["presences"], true);
    }
}


/// `Origin: https://discord.com` for the WS handshake (browsers always
/// send it; its absence reads as a non-browser client).
fn http_origin() -> tokio_tungstenite::tungstenite::http::HeaderValue {
    tokio_tungstenite::tungstenite::http::HeaderValue::from_static("https://discord.com")
}
fn http_ua(ua: &str) -> std::result::Result<tokio_tungstenite::tungstenite::http::HeaderValue, tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue> {
    tokio_tungstenite::tungstenite::http::HeaderValue::from_str(ua)
}

enum ConnOutcome {
    CleanShutdown,
    ReconnectFresh,
    ReconnectResume,
}

async fn run_connection_loop(
    token: String,
    is_bot: bool,
    state: Arc<AppState>,
    _outbound_tx: mpsc::UnboundedSender<Outbound>,
    outbound_rx: &mut mpsc::UnboundedReceiver<Outbound>,
    intent_set: u32,
) -> Result<ConnOutcome, GatewayError> {
    state.set_connection_status(crate::state::ConnectionStatus::Connecting).await;
    tracing::info!(url = GATEWAY_URL, "connecting to gateway");

    // RESUME must go to the gateway URL Discord handed us in READY
    // (`resume_gateway_url`) - resuming against a stale host is the classic
    // cause of invalid-session loops.
    let session = state.session_snapshot().await;
    let ws_url = session
        .as_ref()
        .map(|s| s.resume_gateway_url.clone())
        .filter(|u| u.starts_with("wss://"))
        .unwrap_or_else(|| GATEWAY_URL.to_string());
    tracing::info!(url = %ws_url, "connecting");

    // Browser-shaped handshake: a real browser always sends `Origin` on a
    // websocket upgrade, and the UA must match the one the REST layer uses
    // (and the one claimed in IDENTIFY properties) or the session reads as
    // an inconsistent client to risk systems.
    let ua = if is_bot {
        crate::identity::bot_user_agent()
    } else {
        crate::identity::web_user_agent()
    };
    // `into_client_request` fills in the required handshake headers
    // (Host, Connection, Upgrade, Sec-WebSocket-Key, Sec-WebSocket-Version);
    // a hand-built Request without them is rejected by the server.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| anyhow!("gateway request build: {e}"))?;
    let headers = request.headers_mut();
    headers.insert("Origin", http_origin());
    if let Ok(v) = http_ua(&ua) {
        headers.insert("User-Agent", v);
    }
    let (ws, _) = tokio_tungstenite::connect_async(request).await?;
    let (mut sink, mut stream) = ws.split();
    let mut zlib = GatewayZlib::new();
    use futures_util::{SinkExt, StreamExt};

    // Wait for HELLO.
    let hello = wait_for_hello(&mut stream, &mut zlib).await?;
    let heartbeat_interval = hello.heartbeat_interval;
    tracing::info!(interval_ms = heartbeat_interval, "HELLO received");

    // Send IDENTIFY or RESUME.
    let last_seq = state.last_seq().await;
    let identify_or_resume = match (&session, last_seq) {
        (Some(s), Some(seq)) => {
            let resume_payload = serde_json::json!({
                "op": GatewayOp::Resume as u8,
                "d": {
                    "token": token,
                    "session_id": s.session_id,
                    "seq": seq,
                }
            });
            resume_payload
        }
        _ => {
            // Bot accounts reject the user-client fields (capabilities,
            // client_state, rich properties) with an invalid-session close,
            // so they get the minimal payload everyone accepts. User
            // sessions get the full web-client shape with properties that
            // match the OS, the User-Agent and the browser we claim
            // elsewhere (see src/identity.rs).
            
            if is_bot {
                serde_json::json!({
                    "op": GatewayOp::Identify as u8,
                    "d": {
                        "token": token,
                        "intents": intent_set,
                        "properties": crate::identity::bot_identify_properties(),
                    }
                })
            } else {
                serde_json::json!({
                    "op": GatewayOp::Identify as u8,
                    "d": {
                        "token": token,
                        "capabilities": CLIENT_CAPABILITIES,
                        "properties": crate::identity::web_identify_properties(),
                        "presence": {
                            "status": state.own_status(),
                            "since": 0,
                            "activities": [],
                            "afk": false
                        },
                        "compress": false,
                        "large_threshold": 250,
                        "intents": intent_set,
                        "client_state": {
                            "guild_versions": true,
                            "api_leg_versions": true,
                        }
                    }
                })
            }
        }
    };
    sink.send(WsMessage::Binary(bytes::Bytes::from(identify_or_resume.to_string().into_bytes()))).await?;
    state.set_connection_status(crate::state::ConnectionStatus::Connected).await;

    // Heartbeat task: fires every interval. The heartbeat task sends op 1 with
    // the last known sequence number (or null if none yet).
    let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel::<()>(4);
    let hb_state = state.clone();
    let hb_sink = heartbeat_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(heartbeat_interval.max(500)));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await; // immediate first tick — Discord recommends sending
                              // a heartbeat immediately on connect.
        loop {
            let _ = hb_sink.send(()).await;
            interval.tick().await;
            // If the state's connection status is not Connected, exit.
            if hb_state.connection_status().await != crate::state::ConnectionStatus::Connected {
                break;
            }
        }
    });

    state.mark_heartbeat_ack().await;
    let mut shutdown = false;
    let mut should_resume = false;

    loop {
        tokio::select! {
            biased;
            _ = heartbeat_rx.recv() => {
                if !state.heartbeat_acked() {
                    // The ACK may already be sitting in the socket buffer
                    // behind frames we simply have not read yet (the UI
                    // thread and this loop share one runtime; a burst of
                    // image traffic used to delay reads long enough to
                    // trip this check while the connection was perfectly
                    // healthy). Give the stream a short, bounded chance to
                    // deliver the ACK before declaring the session dead.
                    let mut revived = false;
                    if let Ok(Some(Ok(frame))) = tokio::time::timeout(
                        Duration::from_secs(5),
                        stream.next(),
                    ).await {
                        match frame {
                            WsMessage::Binary(b) => {
                                if let Ok(Some(decoded)) = zlib.push_bytes(&b) {
                                    match handle_decoded(&decoded, &state, &mut sink).await {
                                        Ok(Flow::Continue) => revived = state.heartbeat_acked(),
                                        other => { tracing::warn!(?other, "frame during ACK grace ended the connection"); }
                                    }
                                }
                            }
                            WsMessage::Text(t) => {
                                match handle_decoded(t.as_bytes(), &state, &mut sink).await {
                                    Ok(Flow::Continue) => revived = state.heartbeat_acked(),
                                    other => { tracing::warn!(?other, "text frame during ACK grace"); }
                                }
                            }
                            WsMessage::Ping(p) => {
                                let _ = sink.send(WsMessage::Pong(p)).await;
                            }
                            _ => {}
                        }
                    }
                    if !revived {
                        tracing::warn!(
                            "no heartbeat ACK after grace - session presumed dead, RESUME"
                        );
                        should_resume = true;
                        break;
                    }
                    tracing::info!("heartbeat ACK arrived during grace period");
                }
                let seq = state.last_seq().await;
                let hb = serde_json::json!({
                    "op": GatewayOp::Heartbeat as u8,
                    "d": seq.map(|n| n.0).unwrap_or(0),
                });
                if sink.send(WsMessage::Binary(bytes::Bytes::from(hb.to_string().into_bytes()))).await.is_err() {
                    tracing::warn!("heartbeat send failed - reconnecting");
                    should_resume = true;
                    break;
                }
                state.reset_heartbeat_ack();
            }
            Some(cmd) = outbound_rx.recv() => {
                match cmd {
                    Outbound::Connect { .. } => {
                        // Already connected: a redundant Connect is a no-op.
                    }
                    Outbound::Reconnect => {
                        // Already connected: nothing to reconnect.
                    }
                    Outbound::RequestMembers { guild_id } => {
                        // op 8: the member list + presences arrive as
                        // GUILD_MEMBERS_CHUNK dispatches (see events.rs).
                        let req = request_members_payload(guild_id);
                        if sink
                            .send(WsMessage::Binary(bytes::Bytes::from(req.to_string().into_bytes())))
                            .await
                            .is_err()
                        {
                            tracing::warn!("member request send failed (disconnected?)");
                        }
                    }
                    Outbound::SetPresence { status, afk } => {
                        // Mirror the requested status locally too.
                        state.set_own_status(&status);
                        let p = serde_json::json!({
                            "op": GatewayOp::PresenceUpdate as u8,
                            "d": {
                                "since": 0,
                                "activities": [],
                                "status": status,
                                "afk": afk,
                            }
                        });
                        let _ = sink.send(WsMessage::Binary(bytes::Bytes::from(p.to_string().into_bytes()))).await;
                    }
                    Outbound::Shutdown => {
                        shutdown = true;
                        break;
                    }
                }
            }
            ws_msg = stream.next() => {
                match ws_msg {
                    Some(Ok(WsMessage::Binary(b))) => {
                        let b = b.to_vec();
                        match zlib.push_bytes(&b) {
                            Ok(Some(decoded)) => match handle_decoded(&decoded, &state, &mut sink).await {
                                Ok(Flow::Continue) => {}
                                Ok(Flow::ReconnectFresh) => { break; }
                                Ok(Flow::ReconnectResume) => { should_resume = true; break; }
                                Err(e) => tracing::warn!(error = %e, "dispatch handler error"),
                            },
                            Ok(None) => { /* waiting for more bytes */ }
                            Err(e) => {
                                tracing::error!(error = %e, "zlib decode error - reconnecting fresh");
                                break;
                            }
                        }
                    }
                    Some(Ok(WsMessage::Text(t))) => {
                        // Discord's gateway is binary-encoded but the server may send text frames too.
                        match handle_decoded(t.as_bytes(), &state, &mut sink).await {
                            Ok(Flow::Continue) => {}
                            Ok(Flow::ReconnectFresh) => { break; }
                            Ok(Flow::ReconnectResume) => { should_resume = true; break; }
                            Err(e) => tracing::warn!(error = %e, "dispatch handler error (text frame)"),
                        }
                    }
                    Some(Ok(WsMessage::Ping(p))) => {
                        let _ = sink.send(WsMessage::Pong(p)).await;
                    }
                    Some(Ok(WsMessage::Close(c))) => {
                        tracing::info!(?c, "server-initiated close");
                        let code = c.map(|cf| u16::from(cf.code)).unwrap_or(0);
                        // 4xxx → close per Discord's table.
                        if code == 4004 { return Err(GatewayError::AuthFailed); }
                        if code == 4014 { return Err(GatewayError::DisallowedIntent); }
                        if code == 4009 { return Ok(ConnOutcome::ReconnectFresh); } // session timed out
                        if (4000..5000).contains(&code) {
                            // 4xxx errors → RESUME per Discord docs.
                            return Ok(ConnOutcome::ReconnectResume);
                        }
                        // Plain 1000 close (e.g. after invalid session) - fresh identify.
                        return Ok(ConnOutcome::ReconnectFresh);
                    }
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "ws stream error — reconnecting");
                        should_resume = true;
                        break;
                    }
                    None => {
                        tracing::info!("ws stream closed — reconnecting");
                        should_resume = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    drop(heartbeat_tx);
    let _ = sink.close().await;
    state.set_connection_status(crate::state::ConnectionStatus::Disconnected).await;
    if shutdown { return Ok(ConnOutcome::CleanShutdown); }
    if should_resume { Ok(ConnOutcome::ReconnectResume) } else { Ok(ConnOutcome::ReconnectFresh) }
}

#[derive(Debug, Deserialize)]
struct HelloPayload {
    heartbeat_interval: u64,
}

async fn wait_for_hello(
    stream: &mut WsStreamHalf,
    zlib: &mut GatewayZlib,
) -> Result<HelloPayload, GatewayError> {
    use futures_util::StreamExt;
    let timeout = Duration::from_secs(15);
    let raw = tokio::time::timeout(timeout, stream.next()).await
        .map_err(|_| anyhow!("HELLO timeout"))?;
    let raw = raw.ok_or_else(|| anyhow!("stream closed before HELLO"))??;
    let bytes: Vec<u8> = match raw {
        WsMessage::Binary(b) => b.to_vec(),
        WsMessage::Text(t) => t.as_bytes().to_vec(),
        _ => return Err(anyhow!("unexpected frame type during HELLO").into()),
    };
    // zlib-stream: append, decode.
    if let Some(decoded) = zlib.push_bytes(&bytes)? {
        let v: Value = serde_json::from_slice(&decoded)?;
        let op = v["op"].as_u64();
        if op != Some(GatewayOp::Hello as u64) {
            return Err(anyhow!("expected HELLO, got op={op:?}").into());
        }
        let h: HelloPayload = serde_json::from_value(v["d"].clone())?;
        return Ok(h);
    }
    Err(anyhow!("HELLO did not produce a complete zlib payload (unexpected)").into())
}

/// What the connection loop should do after handling a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    ReconnectFresh,
    ReconnectResume,
}

/// Dispatch a single decoded JSON gateway message.
async fn handle_decoded(
    bytes: &[u8],
    state: &Arc<AppState>,
    sink: &mut WsSink,
) -> Result<Flow, GatewayError> {
    use futures_util::SinkExt;
    let v: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, len = bytes.len(), "json decode failed for gateway frame");
            return Ok(Flow::Continue);
        }
    };
    let op = v["op"].as_u64();
    match op {
        Some(0) => {
            // Dispatch event.
            let seq = v["s"].as_u64().unwrap_or(0);
            let event_name = v["t"].as_str().unwrap_or("");
            state.set_last_seq(crate::model::Snowflake::from_u64(seq)).await;
            let d = v["d"].clone();
            let event = parse_dispatch(event_name, d);
            state.dispatch_event(event).await;
        }
        Some(11) => {
            // Heartbeat ACK — the heartbeat task will see this on the next tick.
            state.mark_heartbeat_ack().await;
        }
        Some(1) => {
            // Server-initiated heartbeat request — send op 1 immediately.
            let hb = serde_json::json!({
                "op": GatewayOp::Heartbeat as u8,
                "d": state.last_seq().await.map(|n| n.0).unwrap_or(0),
            });
            let _ = sink.send(WsMessage::Binary(bytes::Bytes::from(hb.to_string().into_bytes()))).await;
        }
        Some(7) => {
            tracing::info!("server requested RECONNECT - closing for RESUME");
            return Ok(Flow::ReconnectResume);
        }
        Some(9) => {
            let resume = v["d"].as_bool().unwrap_or(false);
            if resume {
                tracing::info!("invalid session (resume=true) - reconnecting with RESUME");
                return Ok(Flow::ReconnectResume);
            } else {
                tracing::info!("invalid session (resume=false) - reconnecting fresh");
                state.clear_session().await;
                return Ok(Flow::ReconnectFresh);
            }
        }
        Some(_) => {
            // Ops we do not act on (presence updates, guild subscriptions, ...).
        }
        None => {
            tracing::warn!(raw = %v, "gateway frame missing op");
        }
    }
    Ok(Flow::Continue)
}
