//! Shared app state. Holds the in-memory cache of guilds/channels/messages,
//! the current selection (guild/channel), the gateway session info, the
//! inbound event channel the UI polls each frame, presence + unread
//! tracking, and the user's own status.
//!
//! Bounded memory policy: keep up to `MAX_MESSAGES_PER_CHANNEL` messages
//! per channel (Discord's own client caps at 50 by default). User records
//! are LRU-cached to `MAX_CACHED_USERS` (5000).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use serde::Deserialize as _;
use tokio::sync::mpsc;

use crate::gateway::events::Event;
use crate::model::{Channel, Guild, Message, Snowflake, User};

/// Process-wide singleton handle for AppState. Allows tokio tasks to fetch
/// state without an Arc clone handoff through every callsite.
static GLOBAL: OnceCell<Arc<AppState>> = OnceCell::new();

pub fn install_global(s: Arc<AppState>) -> Result<(), Arc<AppState>> {
    GLOBAL.set(s)
}

pub fn global() -> Option<Arc<AppState>> {
    GLOBAL.get().cloned()
}

pub const MAX_MESSAGES_PER_CHANNEL: usize = 100;
pub const MAX_CACHED_USERS: usize = 5_000;
/// How long a typing indicator stays alive after TYPING_START.
pub const TYPING_WINDOW: Duration = Duration::from_secs(8);

/// Generate a locally-unique nonce for optimistic message sends. Uses the
/// OS RNG (via the `getrandom` crate) and stays inside the Discord snowflake
/// range so it can double as the temporary id of the pending message.
pub fn new_nonce() -> Snowflake {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    let rand = u64::from_le_bytes(b) & 0x00FF_FFFF_FFFF_FFFF; // 56 bits
    let ctr = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Mix in a counter so two sends within the same nanosecond can never
    // collide (which would make the second echo dedupe into the first).
    Snowflake(rand ^ (ctr.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

/// A draft parked by a failed send, plus the human-readable error, waiting
/// for the UI thread to pick them up on the next frame.
#[derive(Debug, Clone)]
struct FailedSend {
    error: String,
    /// Text to restore into the composer (the unsent message).
    draft: String,
    /// Reply target to restore into the composer.
    reply_to: Option<Snowflake>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    AuthFailed,
    DisallowedIntent,
    Reconnecting,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub resume_gateway_url: String,
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
}

/// Entry in the users LRU. We keep the original User + a last-touched Instant
/// so the eviction pass knows what to drop.
#[derive(Debug, Clone)]
pub struct UserEntry {
    pub user: User,
    pub last_touched: Instant,
}

/// A live typing indicator.
#[derive(Debug, Clone)]
pub struct TypingUser {
    pub user_id: Snowflake,
    pub name: String,
    pub since: Instant,
}

pub struct AppState {
    connection_status: ArcSwap<ConnectionStatus>,
    session: ArcSwap<Option<SessionInfo>>,
    last_seq: ArcSwap<Option<Snowflake>>,
    heartbeat_ack: ArcSwap<bool>,
    reconnect_requested: ArcSwap<bool>,

    // UI side-channels.
    settings_open: ArcSwap<bool>,

    // Locks — used for read-heavy patterns.
    pub guilds: RwLock<Vec<Guild>>,
    pub channels: RwLock<Vec<Channel>>, // all known channels (per-guild + DMs)
    pub current_user: RwLock<Option<User>>,
    pub users: RwLock<VecDeque<UserEntry>>,
    selection: ArcSwap<Selection>,

    // Per-channel message cache: switching channels is instant when the
    // history was already fetched, and events update the right channel.
    messages: RwLock<HashMap<Snowflake, VecDeque<Message>>>,
    // Channels whose history has been fetched at least once.
    fetched: RwLock<std::collections::HashSet<Snowflake>>,
    // Per-channel failed sends: the composer picks these up on its next
    // frame and restores the draft + shows the error. Never auto-retried.
    failed_sends: RwLock<HashMap<Snowflake, FailedSend>>,

    // user_id -> "online" | "idle" | "dnd" | "offline"
    presences: RwLock<HashMap<Snowflake, String>>,
    // The status we requested via the gateway (op 3).
    own_status: ArcSwap<String>,
    // Unread message counts per channel (cleared when the channel is read).
    unread: RwLock<HashMap<Snowflake, u32>>,
    // Unread *mention* counts per channel.
    mentions: RwLock<HashMap<Snowflake, u32>>,
    // Live typing indicators per channel.
    typing: RwLock<HashMap<Snowflake, Vec<TypingUser>>>,
    // Member user-ids per guild (from GUILD_CREATE / op-8 chunks / REST).
    guild_members: RwLock<HashMap<Snowflake, Vec<Snowflake>>>,
    // Pinned messages per channel (from GET /channels/{id}/pins).
    pins: RwLock<HashMap<Snowflake, Vec<Message>>>,
    // Channels with pin updates the user has not viewed yet (red dot on
    // the header pins button).
    pins_unread: RwLock<std::collections::HashSet<Snowflake>>,
    // True when the gateway had to drop privileged intents (presence data
    // unavailable) - surfaced in the UI so the user knows why dots are gray.
    intents_limited: ArcSwap<bool>,

    // ── v0.2 additions ──
    // Send lock: Some(reason) = the sender worker refuses every send
    // (Discord blocked/captcha'd us). Only the user clears it (Retry).
    send_lock: RwLock<Option<String>>,
    // user_id -> full presence (status + activities) for member rows and
    // profile popups.
    presence_full: RwLock<HashMap<Snowflake, crate::model::PresenceUpdate>>,
    // guild_id -> connected voice states (user_id -> channel_id).
    voice_states: RwLock<HashMap<Snowflake, HashMap<Snowflake, Option<Snowflake>>>>,
    // guild_id -> custom emoji list (picker, mentions rendering).
    guild_emojis: RwLock<HashMap<Snowflake, Vec<crate::model::Emoji>>>,
    // message_id -> referenced (replied-to) message fetched out-of-band.
    referenced_messages: RwLock<HashMap<Snowflake, Message>>,
    // url -> YouTube oEmbed metadata (title, author, thumbnail).
    oembed: RwLock<HashMap<String, crate::model::OEmbedInfo>>,
    // user_id -> fetched profile (bio, pronouns) for profile popups.
    profiles: RwLock<HashMap<Snowflake, crate::model::UserProfile>>,
    // guild_id -> scheduled events.
    events: RwLock<HashMap<Snowflake, Vec<crate::model::ScheduledEvent>>>,
    // Pinned DM channel ids (session mirror of the config list).
    pinned_dms: RwLock<std::collections::HashSet<Snowflake>>,
    // Relationships (friends) page state.
    relationships_len: RwLock<Option<usize>>,
    relationships_unavailable: RwLock<Option<String>>,

    event_tx: mpsc::UnboundedSender<Event>,
}

impl AppState {
    pub fn new() -> Self {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        Self::with_sender(event_tx)
    }

    /// Construct with an externally-supplied event sender so the UI can keep
    /// the receiver end and poll for events each frame.
    pub fn with_event_channel(tx: mpsc::UnboundedSender<Event>) -> Self {
        Self::with_sender(tx)
    }

    fn with_sender(event_tx: mpsc::UnboundedSender<Event>) -> Self {
        Self {
            connection_status: ArcSwap::from_pointee(ConnectionStatus::Disconnected),
            session: ArcSwap::from_pointee(None),
            last_seq: ArcSwap::from_pointee(None),
            heartbeat_ack: ArcSwap::from_pointee(true),
            reconnect_requested: ArcSwap::from_pointee(false),
            settings_open: ArcSwap::from_pointee(false),
            guilds: RwLock::new(Vec::new()),
            channels: RwLock::new(Vec::new()),
            current_user: RwLock::new(None),
            users: RwLock::new(VecDeque::with_capacity(MAX_CACHED_USERS)),
            selection: ArcSwap::from_pointee(Selection::default()),
            messages: RwLock::new(HashMap::new()),
            fetched: RwLock::new(std::collections::HashSet::new()),
            failed_sends: RwLock::new(HashMap::new()),
            presences: RwLock::new(HashMap::new()),
            own_status: ArcSwap::from_pointee("online".to_string()),
            unread: RwLock::new(HashMap::new()),
            mentions: RwLock::new(HashMap::new()),
            typing: RwLock::new(HashMap::new()),
            guild_members: RwLock::new(HashMap::new()),
            pins: RwLock::new(HashMap::new()),
            pins_unread: RwLock::new(std::collections::HashSet::new()),
            intents_limited: ArcSwap::from_pointee(false),
            send_lock: RwLock::new(None),
            presence_full: RwLock::new(HashMap::new()),
            voice_states: RwLock::new(HashMap::new()),
            guild_emojis: RwLock::new(HashMap::new()),
            referenced_messages: RwLock::new(HashMap::new()),
            oembed: RwLock::new(HashMap::new()),
            profiles: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            pinned_dms: RwLock::new(std::collections::HashSet::new()),
            relationships_len: RwLock::new(None),
            relationships_unavailable: RwLock::new(None),
            event_tx,
        }
    }

    pub fn request_settings_toggle(&self) {
        let cur = **self.settings_open.load();
        self.settings_open.store(Arc::new(!cur));
    }
    pub fn settings_open(&self) -> bool {
        **self.settings_open.load()
    }
    pub fn set_settings_open(&self, open: bool) {
        self.settings_open.store(Arc::new(open));
    }

    pub fn event_sender(&self) -> mpsc::UnboundedSender<Event> {
        self.event_tx.clone()
    }

    // ── Status / session ──

    pub async fn connection_status(&self) -> ConnectionStatus {
        **self.connection_status.load()
    }
    pub async fn set_connection_status(&self, status: ConnectionStatus) {
        self.connection_status.store(Arc::new(status));
    }
    pub fn connection_status_sync(&self) -> ConnectionStatus {
        **self.connection_status.load()
    }
    pub async fn session_snapshot(&self) -> Option<SessionInfo> {
        let session = self.session.load_full();
        (*session).clone().map(|s| SessionInfo {
            session_id: s.session_id,
            resume_gateway_url: s.resume_gateway_url,
        })
    }
    pub async fn set_session(&self, session_id: String, resume_gateway_url: String) {
        self.session.store(Arc::new(Some(SessionInfo { session_id, resume_gateway_url })));
    }
    pub async fn clear_session(&self) {
        self.session.store(Arc::new(None));
        self.last_seq.store(Arc::new(None));
    }
    pub async fn last_seq(&self) -> Option<Snowflake> {
        **self.last_seq.load()
    }
    pub async fn set_last_seq(&self, n: Snowflake) {
        self.last_seq.store(Arc::new(Some(n)));
    }
    pub async fn mark_heartbeat_ack(&self) {
        self.heartbeat_ack.store(Arc::new(true));
    }
    pub fn heartbeat_acked(&self) -> bool {
        **self.heartbeat_ack.load()
    }
    pub fn reset_heartbeat_ack(&self) {
        self.heartbeat_ack.store(Arc::new(false));
    }
    pub async fn request_reconnect(&self) {
        self.reconnect_requested.store(Arc::new(true));
    }
    pub async fn selection(&self) -> Selection {
        self.selection.load().as_ref().clone()
    }
    pub fn selection_sync(&self) -> Selection {
        self.selection.load().as_ref().clone()
    }
    pub async fn set_selection(&self, s: Selection) {
        self.set_selection_sync(s);
    }
    pub fn set_selection_sync(&self, s: Selection) {
        self.selection.store(Arc::new(s));
        if let Some(cid) = self.selection.load().channel_id {
            self.mark_read(cid);
        }
    }

    // ── Privileged intents ──

    pub fn set_intents_limited(&self, limited: bool) {
        self.intents_limited.store(Arc::new(limited));
    }
    pub fn intents_limited(&self) -> bool {
        **self.intents_limited.load()
    }

    // ── Messages (per-channel cache) ──

    pub fn messages_for(&self, channel_id: Snowflake) -> Vec<Message> {
        self.messages
            .read()
            .get(&channel_id)
            .map(|d| d.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn set_messages(&self, channel_id: Snowflake, msgs: Vec<Message>) {
        let mut deque: VecDeque<Message> = msgs.into();
        while deque.len() > MAX_MESSAGES_PER_CHANNEL {
            deque.pop_front();
        }
        self.fetched.write().insert(channel_id);
        // Only overwrite if the cache doesn't already hold newer data
        // (a live event may have arrived before the REST response).
        let mut m = self.messages.write();
        let merged = match m.get(&channel_id) {
            Some(existing) if !existing.is_empty() && existing.back().map(|b| b.id.0).unwrap_or(0) > deque.back().map(|b| b.id.0).unwrap_or(0) => {
                // REST returned stale data; keep the live cache.
                return;
            }
            _ => deque,
        };
        m.insert(channel_id, merged);
    }

    pub fn mark_fetched(&self, channel_id: Snowflake) {
        self.fetched.write().insert(channel_id);
    }
    pub fn is_fetched(&self, channel_id: Snowflake) -> bool {
        self.fetched.read().contains(&channel_id)
    }

    fn push_message(&self, msg: Message) {
        let cid = msg.channel_id;
        let mut m = self.messages.write();
        let deque = m.entry(cid).or_default();
        // Nonce resolution: when the gateway delivers MESSAGE_CREATE for a
        // message we sent optimistically, replace the pending copy instead
        // of appending (the pending copy has a different temporary id, so
        // id-based dedupe alone would render the message twice).
        if let Some(n) = &msg.nonce {
            if let Some(idx) = deque.iter().position(|x| x.nonce.as_deref() == Some(n.as_str())) {
                deque[idx] = msg;
                return;
            }
        }
        // De-duplicate by id (REST + gateway can both deliver).
        if deque.iter().any(|x| x.id == msg.id) {
            return;
        }
        deque.push_back(msg);
        while deque.len() > MAX_MESSAGES_PER_CHANNEL {
            deque.pop_front();
        }
    }

    // ── Optimistic (pending) messages ──

    /// Insert the local echo of a message we just sent. Keyed by the nonce;
    /// `resolve_pending` / gateway MESSAGE_CREATE will replace it.
    pub fn insert_pending_message(&self, channel_id: Snowflake, msg: &Message) {
        let mut m = self.messages.write();
        let deque = m.entry(channel_id).or_default();
        deque.push_back(msg.clone());
        while deque.len() > MAX_MESSAGES_PER_CHANNEL {
            deque.pop_front();
        }
    }

    /// Replace the pending echo with the real message (REST response).
    pub fn resolve_pending(&self, channel_id: Snowflake, nonce: &str, real: Message) {
        let mut m = self.messages.write();
        if let Some(deque) = m.get_mut(&channel_id) {
            if let Some(idx) = deque.iter().position(|x| x.nonce.as_deref() == Some(nonce)) {
                deque[idx] = real;
                return;
            }
        }
        // The echo is gone (history refetch replaced it): fall back to a
        // plain push, with the usual id dedupe.
        drop(m);
        self.push_message(real);
    }

    /// Remove the pending echo of a failed send and park the draft + error
    /// for the composer to restore. Called only from the failed REST task;
    /// there is no automatic retry.
    pub fn fail_pending(&self, channel_id: Snowflake, nonce: &str, error: String) {
        let mut m = self.messages.write();
        let mut draft = String::new();
        let mut reply_to = None;
        if let Some(deque) = m.get_mut(&channel_id) {
            if let Some(idx) = deque.iter().position(|x| x.nonce.as_deref() == Some(nonce)) {
                let pending = deque.remove(idx).unwrap_or_default();
                draft = pending.content;
                reply_to = pending.message_reference.and_then(|r| r.message_id);
            }
        }
        drop(m);
        self.failed_sends
            .write()
            .insert(channel_id, FailedSend { error, draft, reply_to });
        let _ = self.event_tx.send(Event::RepaintRequested);
    }

    /// Take (consume) a parked failure: (error, draft, reply_to).
    pub fn take_failed_send(&self, channel_id: Snowflake) -> Option<(String, String, Option<Snowflake>)> {
        self.failed_sends
            .write()
            .remove(&channel_id)
            .map(|f| (f.error, f.draft, f.reply_to))
    }

    pub fn current_user(&self) -> Option<User> {
        self.current_user.read().clone()
    }

    // ── Presence ──

    pub fn presence(&self, user_id: Snowflake) -> Option<String> {
        self.presences.read().get(&user_id).cloned()
    }
    pub fn set_presence(&self, user_id: Snowflake, status: &str) {
        self.presences.write().insert(user_id, status.to_string());
    }
    pub fn own_status(&self) -> String {
        (**self.own_status.load()).clone()
    }
    pub fn set_own_status(&self, status: &str) {
        self.own_status.store(Arc::new(status.to_string()));
    }
    /// Request a presence change: mirrors locally immediately and emits an
    /// internal event the app layer turns into a gateway op-3 update.
    pub fn request_presence(&self, status: &str) {
        self.set_own_status(status);
        let _ = self.event_tx.send(Event::PresenceRequested { status: status.to_string() });
    }

    // ── Unread / mentions ──

    pub fn unread_count(&self, channel_id: Snowflake) -> u32 {
        self.unread.read().get(&channel_id).copied().unwrap_or(0)
    }
    pub fn mention_count(&self, channel_id: Snowflake) -> u32 {
        self.mentions.read().get(&channel_id).copied().unwrap_or(0)
    }
    pub fn total_mentions(&self) -> u32 {
        self.mentions.read().values().sum()
    }
    /// Channels with unread activity: (channel_id, unread, mentions).
    pub fn unread_channels(&self) -> Vec<(Snowflake, u32, u32)> {
        let unread = self.unread.read();
        let mentions = self.mentions.read();
        let mut out: Vec<(Snowflake, u32, u32)> = unread
            .iter()
            .map(|(cid, n)| {
                let m = mentions.get(cid).copied().unwrap_or(0);
                (*cid, *n, m)
            })
            .collect();
        out.sort_by_key(|(_, n, m)| std::cmp::Reverse(*n + *m));
        out
    }
    pub fn mark_read(&self, channel_id: Snowflake) {
        self.unread.write().remove(&channel_id);
        self.mentions.write().remove(&channel_id);
    }

    fn bump_unread(&self, msg: &Message) {
        let selected = self.selection_sync().channel_id;
        if selected == Some(msg.channel_id) {
            return; // Reading the channel live = read.
        }
        *self.unread.write().entry(msg.channel_id).or_insert(0) += 1;
        let me = self.current_user.read().as_ref().map(|u| u.id);
        let mentions_me = msg.mention_everyone
            || me.map(|m| msg.mentions.iter().any(|u| u.id == m)).unwrap_or(false);
        if mentions_me {
            *self.mentions.write().entry(msg.channel_id).or_insert(0) += 1;
            // Real notification: toast + desktop banner with sound hint.
            let channel = self
                .channel_by_id(msg.channel_id)
                .map(|c| c.display_name())
                .unwrap_or_else(|| "unknown".into());
            let author = msg.author.display_name().to_string();
            let snippet: String = msg
                .content
                .chars()
                .filter(|c| !c.is_control())
                .take(48)
                .collect();
            crate::notify::mention(&author, &channel, &snippet);
        }
    }

    // ── Members ──

    pub fn guild_member_ids(&self, guild_id: Snowflake) -> Vec<Snowflake> {
        self.guild_members.read().get(&guild_id).cloned().unwrap_or_default()
    }
    pub fn set_guild_members(&self, guild_id: Snowflake, ids: Vec<Snowflake>) {
        self.guild_members.write().insert(guild_id, ids);
    }

    // ── Pins ──

    /// Replace the cached pin list for a channel (from REST).
    pub fn set_pins(&self, channel_id: Snowflake, msgs: Vec<Message>) {
        self.pins.write().insert(channel_id, msgs);
    }
    pub fn pins_for(&self, channel_id: Snowflake) -> Vec<Message> {
        self.pins.read().get(&channel_id).cloned().unwrap_or_default()
    }

    // ── v0.2: pins-unread / send lock / presence / voice / emoji / misc ──

    /// Mark "this channel got new pinned content" (red dot on pins button).
    pub fn pin_updated(&self, channel_id: Snowflake) {
        self.pins_unread.write().insert(channel_id);
    }
    pub fn pins_unread(&self, channel_id: Snowflake) -> bool {
        self.pins_unread.read().contains(&channel_id)
    }
    pub fn clear_pins_unread(&self, channel_id: Snowflake) {
        self.pins_unread.write().remove(&channel_id);
    }

    /// Halt all outbound sends (Discord blocked us). Never auto-cleared.
    pub fn lock_sends(&self, reason: String) {
        let mut lock = self.send_lock.write();
        if lock.is_none() {
            tracing::error!(reason = %reason, "send lock engaged");
        }
        *lock = Some(reason);
    }
    pub fn send_lock_reason(&self) -> Option<String> {
        self.send_lock.read().clone()
    }
    /// User-authorized unlock (the Retry button in the composer banner).
    pub fn clear_send_lock(&self) {
        *self.send_lock.write() = None;
    }

    /// Full presence (status + activities), stored from PRESENCE_UPDATE.
    pub fn set_presence_full(&self, p: &crate::model::PresenceUpdate) {
        self.presences.write().insert(p.user.id, p.status.clone());
        self.presence_full
            .write()
            .insert(p.user.id, p.clone());
    }
    pub fn presence_full(&self, user_id: Snowflake) -> Option<crate::model::PresenceUpdate> {
        self.presence_full.read().get(&user_id).cloned()
    }
    /// First activity line for a user ("Playing Rust", "Listening Spotify").
    pub fn activity_line(&self, user_id: Snowflake) -> Option<String> {
        let p = self.presence_full.read().get(&user_id)?.clone();
        let a = p.activities.first()?;
        let label = match a.kind {
            0 => Some(format!("Playing {}", a.name)).or(a.details.clone()),
            1 => a.details.clone().or(Some(a.name.clone())),
            2 => a
                .details
                .clone()
                .or(Some(format!("Listening to {}", a.name))),
            3 => Some(format!("Watching {}", a.name)).or(a.details.clone()),
            4 => a.state.clone().or(a.details.clone()),
            5 => Some(format!("Competing in {}", a.name)).or(a.details.clone()),
            _ => a.details.clone(),
        }?;
        Some(label)
    }

    /// Voice states: guild_id -> (user_id -> channel_id). `None` = left.
    pub fn apply_voice_state(&self, guild_id: Snowflake, user_id: Snowflake, channel_id: Option<Snowflake>) {
        let mut vs = self.voice_states.write();
        let guild = vs.entry(guild_id).or_default();
        match channel_id {
            Some(cid) => {
                guild.insert(user_id, Some(cid));
            }
            None => {
                guild.remove(&user_id);
            }
        }
    }
    /// Users connected to a voice channel (sidebar mini rows).
    pub fn voice_channel_users(&self, guild_id: Snowflake, channel_id: Snowflake) -> Vec<Snowflake> {
        self.voice_states
            .read()
            .get(&guild_id)
            .map(|g| {
                g.iter()
                    .filter(|(_, cid)| **cid == Some(channel_id))
                    .map(|(uid, _)| *uid)
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn user_voice_channel(&self, guild_id: Snowflake, user_id: Snowflake) -> Option<Snowflake> {
        self.voice_states
            .read()
            .get(&guild_id)
            .and_then(|g| g.get(&user_id).copied().flatten())
    }

    /// Custom emoji of a guild (picker + jumbo rendering).
    pub fn set_guild_emojis(&self, guild_id: Snowflake, emojis: Vec<crate::model::Emoji>) {
        self.guild_emojis.write().insert(guild_id, emojis);
    }
    pub fn guild_emojis(&self, guild_id: Snowflake) -> Vec<crate::model::Emoji> {
        self.guild_emojis
            .read()
            .get(&guild_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Out-of-band fetched replied-to message (reply reference rendering).
    pub fn set_referenced_message(&self, m: &Message) {
        self.referenced_messages.write().insert(m.id, m.clone());
    }
    pub fn referenced_message(&self, id: Snowflake) -> Option<Message> {
        self.referenced_messages.read().get(&id).cloned()
    }

    /// YouTube oEmbed cache.
    pub fn set_oembed(&self, url: &str, info: crate::model::OEmbedInfo) {
        self.oembed.write().insert(url.to_string(), info);
    }
    pub fn oembed(&self, url: &str) -> Option<crate::model::OEmbedInfo> {
        self.oembed.read().get(url).cloned()
    }

    /// Fetched profile (bio/pronouns) for the popup card.
    pub fn set_profile(&self, user_id: Snowflake, p: crate::model::UserProfile) {
        self.profiles.write().insert(user_id, p);
    }
    pub fn profile(&self, user_id: Snowflake) -> Option<crate::model::UserProfile> {
        self.profiles.read().get(&user_id).cloned()
    }

    /// Scheduled events per guild (sidebar Events popup).
    pub fn set_events(&self, guild_id: Snowflake, events: Vec<crate::model::ScheduledEvent>) {
        self.events.write().insert(guild_id, events);
    }
    pub fn events_for(&self, guild_id: Snowflake) -> Vec<crate::model::ScheduledEvent> {
        self.events.read().get(&guild_id).cloned().unwrap_or_default()
    }

    /// Pinned DMs (session mirror of config.pinned_dms).
    pub fn set_pinned_dm(&self, id: Snowflake, pinned: bool) {
        if pinned {
            self.pinned_dms.write().insert(id);
        } else {
            self.pinned_dms.write().remove(&id);
        }
    }
    pub fn pinned_dm(&self, id: Snowflake) -> bool {
        self.pinned_dms.read().contains(&id)
    }
    pub fn set_relationships_len(&self, n: usize) {
        *self.relationships_len.write() = Some(n);
    }
    pub fn relationships_len(&self) -> usize {
        self.relationships_len.read().unwrap_or(0)
    }
    pub fn set_relationships_unavailable(&self, reason: String) {
        *self.relationships_len.write() = Some(0);
        *self.relationships_unavailable.write() = Some(reason);
    }
    pub fn relationships_unavailable(&self) -> Option<String> {
        self.relationships_unavailable.read().clone()
    }
    pub fn init_pinned_dms(&self, ids: &[String]) {
        let mut set = self.pinned_dms.write();
        for id in ids {
            if let Ok(v) = id.parse::<u64>() {
                set.insert(Snowflake::from_u64(v));
            }
        }
    }

    // ── Typing ──

    pub fn set_typing(&self, channel_id: Snowflake, user_id: Snowflake, name: &str) {
        let mut t = self.typing.write();
        let entry = t.entry(channel_id).or_default();
        entry.retain(|u| u.user_id != user_id);
        entry.push(TypingUser {
            user_id,
            name: name.to_string(),
            since: Instant::now(),
        });
    }
    pub fn typing_in(&self, channel_id: Snowflake) -> Vec<String> {
        let mut t = self.typing.write();
        let entry = match t.get_mut(&channel_id) {
            Some(e) => e,
            None => return Vec::new(),
        };
        let now = Instant::now();
        entry.retain(|u| now.duration_since(u.since) < TYPING_WINDOW);
        entry.iter().map(|u| u.name.clone()).collect()
    }

    // ── Event dispatch ──

    pub async fn dispatch_event(&self, event: Event) {
        // Apply state mutations first.
        match &event {
            Event::Ready { d } => {
                self.set_session(d.session_id.clone(), d.resume_gateway_url.clone()).await;
                *self.current_user.write() = Some(d.user.clone());
                // Cache our own user record too: the member panel resolves
                // every row through the user cache, and the bot itself is
                // always a member.
                self.touch_user(&d.user);
                *self.guilds.write() = d.guilds.clone();
                for u in &d.users {
                    self.touch_user(u);
                }
                let mut channels = self.channels.write();
                channels.retain(|c| c.guild_id.is_none()); // drop old guild channels
                channels.extend(d.private_channels_v2.iter().cloned());
                channels.extend(d.private_channels_v1.iter().cloned());
                // Hydrate DM recipient users.
                for ch in d.private_channels_v1.iter().chain(d.private_channels_v2.iter()) {
                    for r in &ch.recipients {
                        self.touch_user(r);
                    }
                }
            }
            Event::Resumed => {}
            Event::GuildCreate { d } => {
                let mut g = self.guilds.write();
                if let Some(idx) = g.iter().position(|x| x.id == d.id) {
                    g[idx] = d.clone();
                } else {
                    g.push(d.clone());
                }
                drop(g);
                // GUILD_CREATE carries the full channel list.
                let mut ch = self.channels.write();
                ch.retain(|c| c.guild_id != Some(d.id));
                for c in &d.channels {
                    let mut c = c.clone();
                    c.guild_id = Some(d.id);
                    ch.push(c);
                }
                drop(ch);
                // Members + presences (present when the intents are granted).
                let mut member_ids: Vec<Snowflake> = Vec::new();
                for m in &d.members {
                    if let Some(u) = &m.user {
                        self.touch_user(u);
                        member_ids.push(u.id);
                    }
                }
                if !member_ids.is_empty() {
                    self.guild_members.write().insert(d.id, member_ids);
                }
                for p in &d.presences {
                    self.set_presence_full(p);
                }
                // Custom emoji for the picker (GUILD_CREATE carries them).
                if !d.emojis.is_empty() {
                    self.set_guild_emojis(d.id, d.emojis.clone());
                }
            }
            Event::GuildMembersChunk { d } => {
                // Op 8 answer: merge the chunk's members into the guild's
                // list (union, idempotent - re-requests are safe) and cache
                // every user record + presence it carries.
                let mut ids = self.guild_member_ids(d.guild_id);
                for m in &d.members {
                    if let Some(u) = &m.user {
                        self.touch_user(u);
                        if !ids.contains(&u.id) {
                            ids.push(u.id);
                        }
                    }
                }
                if !ids.is_empty() {
                    self.guild_members.write().insert(d.guild_id, ids);
                }
                for p in &d.presences {
                    self.set_presence_full(p);
                }
                if d.chunk_index + 1 >= d.chunk_count.max(1) {
                    tracing::debug!(guild = %d.guild_id, "member list complete");
                }
            }
            Event::GuildDelete { d } => {
                let id = Snowflake(d["id"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0));
                if !id.is_empty() {
                    self.guilds.write().retain(|g| g.id != id);
                    self.channels.write().retain(|c| c.guild_id != Some(id));
                }
            }
            Event::ChannelCreate { d } => {
                let mut ch = self.channels.write();
                if let Some(existing) = ch.iter_mut().find(|c| c.id == d.id) {
                    *existing = d.clone();
                } else {
                    ch.push(d.clone());
                }
            }
            Event::ChannelUpdate { d } => {
                let mut ch = self.channels.write();
                if let Some(idx) = ch.iter().position(|c| c.id == d.id) {
                    ch[idx] = d.clone();
                } else {
                    ch.push(d.clone());
                }
            }
            Event::ChannelDelete { d } => {
                let mut ch = self.channels.write();
                ch.retain(|c| c.id != d.id);
            }
            Event::MessageCreate { d } => {
                let cid = d.channel_id;
                let author_name = d.author.display_name().to_string();
                let author_id = d.author.id;
                self.push_message(d.clone());
                self.bump_unread(d);
                self.touch_user(&d.author);
                // Bots never receive a DM list in READY or REST: register DM
                // channels when their first message arrives so the Home
                // list stays in sync.
                if d.guild_id.is_none() && self.channel_by_id(cid).is_none() {
                    let is_me = self
                        .current_user
                        .read()
                        .as_ref()
                        .map(|u| u.id == d.author.id)
                        .unwrap_or(false);
                    if !is_me && !d.author.id.is_empty() {
                        let ch = crate::model::Channel {
                            id: cid,
                            kind: crate::model::ChannelType::Dm,
                            recipients: vec![d.author.clone()],
                            last_message_id: Some(d.id),
                            ..Default::default()
                        };
                        let mut channels = self.channels.write();
                        if !channels.iter().any(|c| c.id == cid) {
                            channels.push(ch);
                        }
                    } else if let Some(rest) = crate::rest::global() {
                        let cid_val = cid;
                        tokio::spawn(async move {
                            match rest.get_channel(cid_val).await {
                                Ok(ch) => {
                                    if let Some(s) = crate::state::global() {
                                        let mut cur = s.channels.write();
                                        if !cur.iter().any(|c| c.id == cid_val) {
                                            cur.push(ch);
                                        }
                                    }
                                }
                                Err(_) => {
                                    // Channel fetch failed; the DM entry is
                                    // skipped and retried on the next event.
                                }
                            }
                        });
                    }
                }
                // Keep last_message_id fresh for DM sorting.
                if d.guild_id.is_none() {
                    if let Some(ch) = self.channels.write().iter_mut().find(|c| c.id == cid) {
                        ch.last_message_id = Some(d.id);
                    }
                }
                // Sending a message cancels your typing indicator.
                let mut t = self.typing.write();
                if let Some(entry) = t.get_mut(&cid) {
                    entry.retain(|u| u.user_id != author_id);
                }
                let _ = author_name;
            }
            Event::MessageUpdate { d } => {
                let mut m = self.messages.write();
                if let Some(deque) = m.get_mut(&d.channel_id) {
                    if let Some(msg) = deque.iter_mut().find(|x| x.id == d.id) {
                        if let Some(content) = &d.content {
                            msg.content = content.clone();
                        }
                        if let Some(edited) = &d.edited_timestamp {
                            msg.edited_timestamp = Some(edited.clone());
                        }
                        if let Some(atts) = &d.attachments {
                            msg.attachments = atts.clone();
                        }
                        if let Some(embeds) = &d.embeds {
                            msg.embeds = embeds.clone();
                        }
                    }
                }
            }
            Event::MessageDelete { d } => {
                let mut m = self.messages.write();
                if let Some(deque) = m.get_mut(&d.channel_id) {
                    deque.retain(|x| x.id != d.id);
                }
            }
            Event::MessageDeleteBulk { d } => {
                let ids: std::collections::HashSet<Snowflake> = d.ids.iter().copied().collect();
                let mut m = self.messages.write();
                if let Some(deque) = m.get_mut(&d.channel_id) {
                    deque.retain(|x| !ids.contains(&x.id));
                }
            }
            Event::TypingStart { d } => {
                let user = d.member.as_ref().and_then(|m| m.user.as_ref().cloned()).or_else(|| self.user(d.user_id));
                let name = user
                    .as_ref()
                    .map(|u| u.display_name().to_string())
                    .unwrap_or_else(|| "someone".to_string());
                self.set_typing(d.channel_id, d.user_id, &name);
            }
            Event::PresenceUpdate { d } => {
                self.set_presence_full(d);
                // Only cache the user record if the payload is complete.
                if let Some(name) = d.user.display_name() {
                    let u = crate::model::User {
                        id: d.user.id,
                        username: d.user.username.clone().unwrap_or_else(|| name.to_string()),
                        global_name: d.user.global_name.clone(),
                        discriminator: d.user.discriminator.clone(),
                        avatar: d.user.avatar.clone(),
                        bot: d.user.bot,
                        ..Default::default()
                    };
                    self.touch_user(&u);
                }
            }
            Event::GuildUpdate { d } => {
                let mut g = self.guilds.write();
                if let Some(idx) = g.iter().position(|x| x.id == d.id) {
                    g[idx] = d.clone();
                }
            }
            Event::ChannelPinsUpdate { d } => {
                // Red dot on the header pins button for this channel.
                if let Some(cid) = d["channel_id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
                    self.pin_updated(Snowflake::from_u64(cid));
                    // Refresh the pins cache too so the popup shows the new pin.
                    if let Some(rest) = crate::rest::global() {
                        let cid = Snowflake::from_u64(cid);
                        tokio::spawn(async move {
                            if let Ok(pins) = rest.get_channel_pins(cid).await {
                                if let Some(s) = crate::state::global() {
                                    s.set_pins(cid, pins);
                                    s.event_sender()
                                        .send(crate::gateway::events::Event::RepaintRequested)
                                        .ok();
                                }
                            }
                        });
                    }
                }
            }
            Event::MessageReactionAdd { d } => self.apply_reaction(d, true),
            Event::MessageReactionRemove { d } => self.apply_reaction(d, false),
            Event::MessageReactionRemoveAll { d: _ } => {}
            Event::MessageReactionRemoveEmoji { d: _ } => {}
            Event::VoiceStateUpdate { d } => {
                if let Some(gid) = d.guild_id {
                    self.apply_voice_state(gid, d.user_id, d.channel_id);
                    if let Some(u) = self.user(d.user_id) {
                        self.touch_user(&u);
                    }
                }
            }
            Event::GuildEmojisUpdate { d } => {
                // {guild_id, emojis: [...]}
                let gid = d["guild_id"].as_str().and_then(|s| s.parse::<u64>().ok());
                let emojis = d["emojis"].as_array().cloned().unwrap_or_default();
                if let Some(gid) = gid {
                    let parsed: Vec<crate::model::Emoji> = emojis
                        .iter()
                        .filter_map(|e| serde_json::from_value(e.clone()).ok())
                        .collect();
                    self.set_guild_emojis(Snowflake::from_u64(gid), parsed);
                }
            }
            Event::VoiceServerUpdate { d: _ } => {}
            Event::UserUpdate { d } => {
                *self.current_user.write() = Some(d.clone());
                self.touch_user(d);
            }
            Event::PresenceRequested { status: _ } => {
                // Internal UI command - handled by the app layer, not state.
            }
            Event::Unknown { .. } => {
                // Events we have not modeled yet: nothing to update.
            }
            Event::RepaintRequested => {
                // Internal signal only; the UI repaints on any event.
            }
        }
        // Forward to UI for repaint.
        let _ = self.event_tx.send(event);
    }

    fn apply_reaction(&self, d: &serde_json::Value, add: bool) {
        let channel_id = match d["channel_id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
            Some(v) => Snowflake::from_u64(v),
            None => return,
        };
        let message_id = match d["message_id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
            Some(v) => Snowflake::from_u64(v),
            None => return,
        };
        let emoji = match crate::model::ReactionEmoji::deserialize(&d["emoji"]) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut m = self.messages.write();
        if let Some(deque) = m.get_mut(&channel_id) {
            if let Some(msg) = deque.iter_mut().find(|x| x.id == message_id) {
                if add {
                    if let Some(r) = msg.reactions.iter_mut().find(|r| r.emoji.content_string() == emoji.content_string()) {
                        r.count += 1;
                    } else {
                        msg.reactions.push(crate::model::Reaction {
                            count: 1,
                            me: false,
                            emoji,
                        });
                    }
                } else {
                    msg.reactions.retain_mut(|r| {
                        if r.emoji.content_string() == emoji.content_string() {
                            r.count = r.count.saturating_sub(1);
                            r.count > 0
                        } else {
                            true
                        }
                    });
                }
            }
        }
    }

    // ── User cache (LRU) ──

    pub fn touch_user(&self, u: &User) {
        let mut g = self.users.write();
        if let Some(idx) = g.iter().position(|e| e.user.id == u.id) {
            g[idx].user = u.clone();
            g[idx].last_touched = Instant::now();
            let entry = g.remove(idx).unwrap();
            g.push_back(entry);
        } else {
            if g.len() >= MAX_CACHED_USERS {
                g.pop_front();
            }
            g.push_back(UserEntry { user: u.clone(), last_touched: Instant::now() });
        }
    }
    pub fn user(&self, id: Snowflake) -> Option<User> {
        let g = self.users.read();
        g.iter().find(|e| e.user.id == id).map(|e| e.user.clone())
    }

    pub fn guild_by_id(&self, id: Snowflake) -> Option<Guild> {
        self.guilds.read().iter().find(|g| g.id == id).cloned()
    }
    pub fn channels_for_guild(&self, guild_id: Snowflake) -> Vec<Channel> {
        self.channels
            .read()
            .iter()
            .filter(|c| c.guild_id == Some(guild_id))
            .cloned()
            .collect()
    }
    pub fn dm_channels(&self) -> Vec<Channel> {
        self.channels
            .read()
            .iter()
            .filter(|c| matches!(c.kind, crate::model::ChannelType::Dm | crate::model::ChannelType::GroupDm))
            .cloned()
            .collect()
    }
    pub fn channel_by_id(&self, id: Snowflake) -> Option<Channel> {
        self.channels.read().iter().find(|c| c.id == id).cloned()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    /// Drive one async state call from a sync test.
    fn tokio_block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(f)
    }

    use super::*;

    fn msg(id: u64, channel: u64, content: &str) -> Message {
        let mut m = Message::default();
        m.id = Snowflake::from_u64(id);
        m.channel_id = Snowflake::from_u64(channel);
        m.content = content.to_string();
        m
    }

    #[test]
    fn per_channel_cache_isolated() {
        let s = AppState::new();
        s.set_messages(Snowflake::from_u64(1), vec![msg(10, 1, "a")]);
        s.set_messages(Snowflake::from_u64(2), vec![msg(20, 2, "b")]);
        assert_eq!(s.messages_for(Snowflake::from_u64(1)).len(), 1);
        assert_eq!(s.messages_for(Snowflake::from_u64(2)).len(), 1);
        assert_eq!(s.messages_for(Snowflake::from_u64(3)).len(), 0);
        assert!(s.is_fetched(Snowflake::from_u64(1)));
        assert!(!s.is_fetched(Snowflake::from_u64(3)));
    }

    #[tokio::test]
    async fn dispatch_message_create_appends_to_channel() {
        let s = AppState::new();
        s.dispatch_event(Event::MessageCreate { d: msg(1, 42, "hello") }).await;
        s.dispatch_event(Event::MessageCreate { d: msg(2, 43, "other channel") }).await;
        assert_eq!(s.messages_for(Snowflake::from_u64(42)).len(), 1);
        assert_eq!(s.messages_for(Snowflake::from_u64(43)).len(), 1);
    }

    #[tokio::test]
    async fn duplicate_message_not_duplicated() {
        let s = AppState::new();
        s.dispatch_event(Event::MessageCreate { d: msg(1, 42, "hello") }).await;
        s.dispatch_event(Event::MessageCreate { d: msg(1, 42, "hello") }).await;
        assert_eq!(s.messages_for(Snowflake::from_u64(42)).len(), 1);
    }

    #[tokio::test]
    async fn unread_bumps_only_for_other_channels() {
        let s = AppState::new();
        s.set_selection_sync(Selection {
            guild_id: None,
            channel_id: Some(Snowflake::from_u64(7)),
        });
        s.dispatch_event(Event::MessageCreate { d: msg(1, 7, "live") }).await;
        s.dispatch_event(Event::MessageCreate { d: msg(2, 9, "away") }).await;
        assert_eq!(s.unread_count(Snowflake::from_u64(7)), 0);
        assert_eq!(s.unread_count(Snowflake::from_u64(9)), 1);
        s.mark_read(Snowflake::from_u64(9));
        assert_eq!(s.unread_count(Snowflake::from_u64(9)), 0);
    }

    #[tokio::test]
    async fn mention_count_for_mentioned_user() {
        let s = AppState::new();
        let mut me = User::default();
        me.id = Snowflake::from_u64(100);
        *s.current_user.write() = Some(me.clone());
        let mut m = msg(1, 9, "hi");
        m.mentions.push(me);
        s.dispatch_event(Event::MessageCreate { d: m }).await;
        assert_eq!(s.mention_count(Snowflake::from_u64(9)), 1);
        assert_eq!(s.total_mentions(), 1);
    }

    #[test]
    fn presence_map_updates() {
        let s = AppState::new();
        s.set_presence(Snowflake::from_u64(5), "idle");
        assert_eq!(s.presence(Snowflake::from_u64(5)).as_deref(), Some("idle"));
        assert_eq!(s.presence(Snowflake::from_u64(6)), None);
    }

    #[test]
    fn typing_expires() {
        let s = AppState::new();
        s.set_typing(Snowflake::from_u64(1), Snowflake::from_u64(2), "alice");
        assert_eq!(s.typing_in(Snowflake::from_u64(1)), vec!["alice".to_string()]);
        // Force-expire by rewinding the clock entry.
        let mut t = s.typing.write();
        t.get_mut(&Snowflake::from_u64(1)).unwrap()[0].since =
            Instant::now() - TYPING_WINDOW - Duration::from_secs(1);
        drop(t);
        assert!(s.typing_in(Snowflake::from_u64(1)).is_empty());
    }

    #[tokio::test]
    async fn stale_rest_response_keeps_live_cache() {
        let s = AppState::new();
        // Gateway delivered a live message (id 50).
        s.dispatch_event(Event::MessageCreate { d: msg(50, 42, "live") }).await;
        // A late REST response only has older messages (id 10).
        s.set_messages(Snowflake::from_u64(42), vec![msg(10, 42, "rest")]);
        let msgs = s.messages_for(Snowflake::from_u64(42));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id, Snowflake::from_u64(50));
    }

    #[tokio::test]
    async fn reaction_add_updates_cached_message() {
        let s = AppState::new();
        s.dispatch_event(Event::MessageCreate { d: msg(1, 42, "react to me") }).await;
        let d = serde_json::json!({
            "channel_id": "42",
            "message_id": "1",
            "emoji": { "name": "🔥" }
        });
        s.apply_reaction(&d, true);
        let msgs = s.messages_for(Snowflake::from_u64(42));
        assert_eq!(msgs[0].reactions.len(), 1);
        assert_eq!(msgs[0].reactions[0].count, 1);
        s.apply_reaction(&d, true);
        assert_eq!(s.messages_for(Snowflake::from_u64(42))[0].reactions[0].count, 2);
        s.apply_reaction(&d, false);
        assert_eq!(s.messages_for(Snowflake::from_u64(42))[0].reactions[0].count, 1);
        s.apply_reaction(&d, false);
        assert_eq!(s.messages_for(Snowflake::from_u64(42))[0].reactions.len(), 0);
    }

    // ── optimistic send / nonce ──

    fn nonce_msg(id: u64, nonce: Option<&str>) -> Message {
        Message {
            id: Snowflake(id),
            channel_id: Snowflake(1),
            author: User::default(),
            content: "hi".into(),
            nonce: nonce.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn gateway_message_replaces_pending_echo_by_nonce() {
        let s = AppState::new();
        s.insert_pending_message(Snowflake(1), &nonce_msg(999, Some("n1")));
        // The gateway copy has a different (real) id but the same nonce.
        let e = crate::gateway::events::Event::MessageCreate {
            d: nonce_msg(1234, Some("n1")),
        };
        tokio_block_on(s.dispatch_event(e));
        let msgs = s.messages_for(Snowflake(1));
        assert_eq!(msgs.len(), 1, "echo replaced, not appended");
        assert_eq!(msgs[0].id.0, 1234);
    }

    #[test]
    fn resolve_pending_swaps_echo_for_real_message() {
        let s = AppState::new();
        s.insert_pending_message(Snowflake(1), &nonce_msg(999, Some("n1")));
        let mut real = nonce_msg(4321, Some("n1"));
        real.content = "hi (server copy)".into();
        s.resolve_pending(Snowflake(1), "n1", real);
        let msgs = s.messages_for(Snowflake(1));
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].id.0, 4321);
        assert_eq!(msgs[0].content, "hi (server copy)");
    }

    #[test]
    fn fail_pending_removes_echo_and_parks_draft() {
        let s = AppState::new();
        let mut echo = nonce_msg(999, Some("n1"));
        echo.content = "unsent text".into();
        s.insert_pending_message(Snowflake(1), &echo);
        s.fail_pending(Snowflake(1), "n1", "boom".into());
        assert!(s.messages_for(Snowflake(1)).is_empty(), "echo removed");
        let (err, draft, _) = s.take_failed_send(Snowflake(1)).expect("parked failure");
        assert_eq!(err, "boom");
        assert_eq!(draft, "unsent text");
        assert!(s.take_failed_send(Snowflake(1)).is_none(), "taken once");
    }

    #[test]
    fn nonces_are_unique() {
        let mut set = std::collections::HashSet::new();
        for _ in 0..1000 {
            assert!(set.insert(new_nonce()));
        }
    }

    #[test]
    fn plain_gateway_message_still_dedupes_by_id() {
        let s = AppState::new();
        let e = crate::gateway::events::Event::MessageCreate {
            d: nonce_msg(7, None),
        };
        tokio_block_on(s.dispatch_event(e.clone()));
        tokio_block_on(s.dispatch_event(e));
        assert_eq!(s.messages_for(Snowflake(1)).len(), 1);
    }

    #[test]
    fn guild_members_chunk_merges_members_and_presences() {
        let s = AppState::new();
        let chunk = crate::gateway::events::Event::GuildMembersChunk {
            d: Box::new(crate::gateway::events::GuildMembersChunkData {
                guild_id: Snowflake(5),
                members: vec![
                    crate::model::Member {
                        user: Some(crate::model::User {
                            id: Snowflake(100),
                            username: "alice".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    crate::model::Member {
                        user: Some(crate::model::User {
                            id: Snowflake(101),
                            username: "bob".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
                presences: vec![crate::model::PresenceUpdate {
                    user: crate::model::PartialUser {
                        id: Snowflake(100),
                        ..Default::default()
                    },
                    status: "online".into(),
                    ..Default::default()
                }],
                chunk_index: 0,
                chunk_count: 1,
                ..Default::default()
            }),
        };
        tokio_block_on(s.dispatch_event(chunk));
        let ids = s.guild_member_ids(Snowflake(5));
        assert_eq!(ids.len(), 2, "both members known");
        assert_eq!(s.user(Snowflake(100)).unwrap().username, "alice");
        assert_eq!(s.presence(Snowflake(100)).as_deref(), Some("online"));
        // A second (re-requested) chunk must not duplicate anyone.
        let chunk2 = crate::gateway::events::Event::GuildMembersChunk {
            d: Box::new(crate::gateway::events::GuildMembersChunkData {
                guild_id: Snowflake(5),
                members: vec![crate::model::Member {
                    user: Some(crate::model::User {
                        id: Snowflake(100),
                        username: "alice".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                chunk_count: 1,
                ..Default::default()
            }),
        };
        tokio_block_on(s.dispatch_event(chunk2));
        assert_eq!(s.guild_member_ids(Snowflake(5)).len(), 2, "merge is idempotent");
    }
}
