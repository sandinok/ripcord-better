//! Gateway event types. Source-of-truth: research/gateway-spec.md §4.
//!
//! We intentionally accept raw `serde_json::Value` for unknown dispatch events
//! so Discord adding new event types doesn't break our deserialization.
//! Only the events we explicitly handle are parsed into typed structs.

use serde_json::Value;

use crate::model::{Channel, Guild, Member, Message, PresenceUpdate, Snowflake, TypingStart, VoiceState};

/// Parsed dispatch event. Discord sends these as op 0 messages with the
/// `t` field set to the event name.
#[derive(Debug, Clone)]
pub enum Event {
    /// The big "you're connected" event. Contains the user record, the
    /// initial guild list (chunks come later via GUILD_CREATE), session_id,
    /// resume_gateway_url, and the private_channels_v2 list.
    Ready { d: Box<ReadyPayload> },
    /// RESUMED — Discord confirmed our replay, missed events delivered.
    Resumed,
    /// A guild's full payload arrived (channels, roles, members chunk,
    /// emojis, presences). Discord sends these in chunks.
    GuildCreate { d: Guild },
    /// A slice of a guild's member list, delivered after an op 8
    /// REQUEST_GUILD_MEMBERS (or lazily for large guilds). Carries member
    /// records plus live presences.
    GuildMembersChunk { d: Box<GuildMembersChunkData> },
    GuildDelete { d: Value },
    GuildUpdate { d: Guild },
    /// A new channel in a guild (or the channel we joined).
    ChannelCreate { d: Channel },
    ChannelUpdate { d: Channel },
    ChannelDelete { d: Channel },
    /// A channel pinned-messages list update (rare).
    ChannelPinsUpdate { d: Value },
    /// A new message arrived.
    MessageCreate { d: Message },
    MessageUpdate { d: PartialMessage },
    MessageDelete { d: MessageDelete },
    MessageDeleteBulk { d: MessageDeleteBulk },
    MessageReactionAdd { d: Value },
    MessageReactionRemove { d: Value },
    MessageReactionRemoveAll { d: Value },
    MessageReactionRemoveEmoji { d: Value },
    TypingStart { d: TypingStart },
    PresenceUpdate { d: PresenceUpdate },
    VoiceStateUpdate { d: VoiceState },
    VoiceServerUpdate { d: VoiceServerUpdate },
    UserUpdate { d: crate::model::User },
    /// INTERNAL (never arrives from the wire): the UI wants to change the
    /// user's own presence; the app layer forwards this to the gateway.
    PresenceRequested { status: String },
    /// INTERNAL (never arrives from the wire): a background task changed
    /// state the UI renders (e.g. a failed send parked a draft); the app
    /// layer answers with a repaint.
    RepaintRequested,
    /// Unknown event — captured for logging only.
    Unknown { name: String, d: Value },
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ReadyPayload {
    pub session_id: String,
    pub resume_gateway_url: String,
    #[serde(default)]
    pub shard: Option<[u32; 2]>,
    #[serde(default)]
    pub v: Option<u32>,
    pub user: crate::model::User,
    #[serde(default)]
    pub guilds: Vec<Guild>,
    #[serde(default, rename = "private_channels")]
    pub private_channels_v1: Vec<Channel>,
    #[serde(default, rename = "private_channels_v2")]
    pub private_channels_v2: Vec<Channel>,
    #[serde(default)]
    pub users: Vec<crate::model::User>,
    #[serde(default)]
    pub merged_members: Vec<Vec<Member>>,
    #[serde(default)]
    pub relationships: Vec<Value>,
    #[serde(default)]
    pub read_state: Value,
    #[serde(default)]
    pub user_settings: Value,
    #[serde(default)]
    pub application: Value,
    #[serde(default)]
    pub session_type: Option<String>,
    #[serde(default)]
    pub geo: Option<String>,
    #[serde(default)]
    pub consents: Value,
}

/// GUILD_MEMBERS_CHUNK payload (op 0, t=GUILD_MEMBERS_CHUNK): a slice of
/// the guild's member list, requested via op 8 (or streamed by Discord for
/// large guilds). `presences` ride along when requested with
/// `presences: true`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GuildMembersChunkData {
    pub guild_id: crate::model::Snowflake,
    #[serde(default)]
    pub members: Vec<crate::model::Member>,
    #[serde(default)]
    pub presences: Vec<crate::model::PresenceUpdate>,
    #[serde(default)]
    pub chunk_index: u32,
    #[serde(default)]
    pub chunk_count: u32,
    #[serde(default)]
    pub not_found: Vec<crate::model::Snowflake>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PartialMessage {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    #[serde(default)]
    pub author: Option<crate::model::User>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub attachments: Option<Vec<crate::model::Attachment>>,
    #[serde(default)]
    pub embeds: Option<Vec<crate::model::Embed>>,
    #[serde(default)]
    pub mentions: Option<Vec<crate::model::User>>,
    #[serde(default)]
    pub flags: Option<u32>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MessageDelete {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct MessageDeleteBulk {
    pub ids: Vec<Snowflake>,
    pub channel_id: Snowflake,
    pub guild_id: Option<Snowflake>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct VoiceServerUpdate {
    pub token: String,
    pub guild_id: Option<Snowflake>,
    pub endpoint: Option<String>,
}

/// Parse a single op-0 dispatch payload given the event name + raw `d`.
pub fn parse_dispatch(event_name: &str, d: Value) -> Event {
    match event_name {
        "READY" => match serde_json::from_value::<ReadyPayload>(d.clone()) {
            Ok(ready) => Event::Ready { d: Box::new(ready) },
            Err(e) => {
                tracing::warn!(error = %e, "READY payload parse error — using fallback");
                // The safest thing for an unparsed READY is to drop it and let
                // the user manually trigger a reconnection. But we still want
                // the event logged with the raw payload.
                Event::Unknown { name: "READY".into(), d }
            }
        },
        "RESUMED" => Event::Resumed,
        "GUILD_CREATE" => match serde_json::from_value::<Guild>(d.clone()) {
            Ok(g) => Event::GuildCreate { d: g },
            Err(e) => {
                tracing::warn!(error = %e, "GUILD_CREATE parse error");
                Event::Unknown { name: "GUILD_CREATE".into(), d }
            }
        },
        "GUILD_DELETE" => Event::GuildDelete { d },
        "GUILD_MEMBERS_CHUNK" => match serde_json::from_value::<GuildMembersChunkData>(d.clone()) {
            Ok(c) => Event::GuildMembersChunk { d: Box::new(c) },
            Err(e) => {
                tracing::warn!(error = %e, "GUILD_MEMBERS_CHUNK parse error");
                Event::Unknown { name: "GUILD_MEMBERS_CHUNK".into(), d }
            }
        },
        "GUILD_UPDATE" => match serde_json::from_value::<Guild>(d.clone()) {
            Ok(g) => Event::GuildUpdate { d: g },
            Err(_) => Event::Unknown { name: "GUILD_UPDATE".into(), d },
        },
        "CHANNEL_CREATE" => match serde_json::from_value::<Channel>(d.clone()) {
            Ok(c) => Event::ChannelCreate { d: c },
            Err(_) => Event::Unknown { name: "CHANNEL_CREATE".into(), d },
        },
        "CHANNEL_UPDATE" => match serde_json::from_value::<Channel>(d.clone()) {
            Ok(c) => Event::ChannelUpdate { d: c },
            Err(_) => Event::Unknown { name: "CHANNEL_UPDATE".into(), d },
        },
        "CHANNEL_DELETE" => match serde_json::from_value::<Channel>(d.clone()) {
            Ok(c) => Event::ChannelDelete { d: c },
            Err(_) => Event::Unknown { name: "CHANNEL_DELETE".into(), d },
        },
        "CHANNEL_PINS_UPDATE" => Event::ChannelPinsUpdate { d },
        "MESSAGE_CREATE" => match serde_json::from_value::<Message>(d.clone()) {
            Ok(m) => Event::MessageCreate { d: m },
            Err(e) => {
                tracing::warn!(error = %e, "MESSAGE_CREATE parse error");
                Event::Unknown { name: "MESSAGE_CREATE".into(), d }
            }
        },
        "MESSAGE_UPDATE" => match serde_json::from_value::<PartialMessage>(d.clone()) {
            Ok(m) => Event::MessageUpdate { d: m },
            Err(_) => Event::Unknown { name: "MESSAGE_UPDATE".into(), d },
        },
        "MESSAGE_DELETE" => match serde_json::from_value::<MessageDelete>(d.clone()) {
            Ok(m) => Event::MessageDelete { d: m },
            Err(_) => Event::Unknown { name: "MESSAGE_DELETE".into(), d },
        },
        "MESSAGE_DELETE_BULK" => match serde_json::from_value::<MessageDeleteBulk>(d.clone()) {
            Ok(m) => Event::MessageDeleteBulk { d: m },
            Err(_) => Event::Unknown { name: "MESSAGE_DELETE_BULK".into(), d },
        },
        "MESSAGE_REACTION_ADD" => Event::MessageReactionAdd { d },
        "MESSAGE_REACTION_REMOVE" => Event::MessageReactionRemove { d },
        "MESSAGE_REACTION_REMOVE_ALL" => Event::MessageReactionRemoveAll { d },
        "MESSAGE_REACTION_REMOVE_EMOJI" => Event::MessageReactionRemoveEmoji { d },
        "TYPING_START" => match serde_json::from_value::<TypingStart>(d.clone()) {
            Ok(t) => Event::TypingStart { d: t },
            Err(_) => Event::Unknown { name: "TYPING_START".into(), d },
        },
        "PRESENCE_UPDATE" => match serde_json::from_value::<PresenceUpdate>(d.clone()) {
            Ok(p) => Event::PresenceUpdate { d: p },
            Err(_) => Event::Unknown { name: "PRESENCE_UPDATE".into(), d },
        },
        "VOICE_STATE_UPDATE" => match serde_json::from_value::<VoiceState>(d.clone()) {
            Ok(v) => Event::VoiceStateUpdate { d: v },
            Err(_) => Event::Unknown { name: "VOICE_STATE_UPDATE".into(), d },
        },
        "VOICE_SERVER_UPDATE" => match serde_json::from_value::<VoiceServerUpdate>(d.clone()) {
            Ok(v) => Event::VoiceServerUpdate { d: v },
            Err(_) => Event::Unknown { name: "VOICE_SERVER_UPDATE".into(), d },
        },
        "USER_UPDATE" => match serde_json::from_value::<crate::model::User>(d.clone()) {
            Ok(u) => Event::UserUpdate { d: u },
            Err(_) => Event::Unknown { name: "USER_UPDATE".into(), d },
        },
        other => Event::Unknown { name: other.into(), d },
    }
}
