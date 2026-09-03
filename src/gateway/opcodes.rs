//! Discord gateway opcodes. Source-of-truth: research/gateway-spec.md §2.
//!
//! Verified 2026-08-30. The DAVE E2EE opcodes (21–31) are voice-gateway only,
//! they don't appear on the main gateway. The undocumented opcodes (5, 13–35)
//! appear in abaddon's source — we keep them as constants for completeness.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum GatewayOp {
    /// Client → Server: heartbeat (also Server → Client as heartbeat req).
    Heartbeat = 1,
    /// Client → Server: IDENTIFY.
    Identify = 2,
    /// Server → Client: presence update.
    PresenceUpdate = 3,
    /// Client → Server: VOICE_STATE_UPDATE.
    VoiceStateUpdate = 4,
    /// Server → Client (undocumented): Voice server ping (abaddon-style).
    VoiceServerPing = 5,
    /// Client → Server: RESUME.
    Resume = 6,
    /// Client → Server: reconnect request.
    Reconnect = 7,
    /// Client → Server: request guild members (gateway v8+).
    RequestGuildMembers = 8,
    /// Server → Client: invalid session.
    InvalidSession = 9,
    /// Server → Client: HELLO.
    Hello = 10,
    /// Server → Client: Heartbeat ACK.
    HeartbeatAck = 11,
    /// Server → Client (undocumented): forwarded guild sync.
    GuildSync = 12,
}

impl GatewayOp {
    pub fn from_u8(n: u8) -> Option<Self> {
        Some(match n {
            1 => Self::Heartbeat,
            2 => Self::Identify,
            3 => Self::PresenceUpdate,
            4 => Self::VoiceStateUpdate,
            5 => Self::VoiceServerPing,
            6 => Self::Resume,
            7 => Self::Reconnect,
            8 => Self::RequestGuildMembers,
            9 => Self::InvalidSession,
            10 => Self::Hello,
            11 => Self::HeartbeatAck,
            12 => Self::GuildSync,
            _ => return None,
        })
    }
}

/// Discord gateway close codes. (See research/gateway-spec.md §6 / §8.)
/// These tell us whether we should re-IDENTIFY, RESUME, or stop entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayCloseCode {
    UnknownError = 4000,
    UnknownOpcode = 4001,
    DecodeError = 4002,
    NotAuthenticated = 4003,
    AuthenticationFailed = 4004,
    AlreadyAuthenticated = 4005,
    InvalidSeq = 4007,
    RateLimited = 4008,
    SessionTimedOut = 4009,
    InvalidShard = 4010,
    ShardingRequired = 4011,
    InvalidApiVersion = 4012,
    InvalidIntents = 4013,
    DisallowedIntents = 4014,
    /// Discord uses 1_0xx for closure frames on the voice gateway.
    VoiceDisconnected = 1000,
    VoiceReconnect = 1012,
}

impl GatewayCloseCode {
    /// Returns true if Discord permits reconnection (with RESUME if seq
    /// is preserved, IDENTIFY fresh otherwise). Returns false for
    /// `AuthenticationFailed` (token bad → don't reconnect).
    pub fn reconnect_allowed(self) -> bool {
        !matches!(self, Self::AuthenticationFailed)
    }
}

pub fn from_close_code(code: u16) -> Option<GatewayCloseCode> {
    use GatewayCloseCode::*;
    Some(match code {
        4000 => UnknownError,
        4001 => UnknownOpcode,
        4002 => DecodeError,
        4003 => NotAuthenticated,
        4004 => AuthenticationFailed,
        4005 => AlreadyAuthenticated,
        4007 => InvalidSeq,
        4008 => RateLimited,
        4009 => SessionTimedOut,
        4010 => InvalidShard,
        4011 => ShardingRequired,
        4012 => InvalidApiVersion,
        4013 => InvalidIntents,
        4014 => DisallowedIntents,
        1000 => VoiceDisconnected,
        1012 => VoiceReconnect,
        _ => return None,
    })
}

/// Gateway intents bitmask. We request the minimum set to keep Discord happy
/// (and to avoid `DisallowedIntents` for privileged intents like GUILD_MEMBERS).
///
/// Verified 2026-08-30 against the Discord docs (research/gateway-spec.md §7):
///   GUILDS                   = 1 << 0
///   GUILD_MEMBERS            = 1 << 1   (privileged)
///   GUILD_MODERATION         = 1 << 2
///   GUILD_EMOJIS_AND_STICKERS = 1 << 3
///   GUILD_INTEGRATIONS       = 1 << 4
///   GUILD_WEBHOOKS           = 1 << 5
///   GUILD_INVITES            = 1 << 6
///   GUILD_VOICE_STATES       = 1 << 7
///   GUILD_PRESENCES          = 1 << 8    (privileged)
///   GUILD_MESSAGES           = 1 << 9
///   GUILD_MESSAGE_REACTIONS  = 1 << 10
///   GUILD_MESSAGE_TYPING     = 1 << 11
///   DIRECT_MESSAGES          = 1 << 12
///   DIRECT_MESSAGE_REACTIONS = 1 << 13
///   DIRECT_MESSAGE_TYPING    = 1 << 14
///   MESSAGE_CONTENT          = 1 << 15  (privileged for bots; user tokens ignore)
pub mod intents {
    pub const GUILDS: u32 = 1 << 0;
    pub const GUILD_MEMBERS: u32 = 1 << 1;
    pub const GUILD_MODERATION: u32 = 1 << 2;
    pub const GUILD_EMOJIS_AND_STICKERS: u32 = 1 << 3;
    pub const GUILD_INTEGRATIONS: u32 = 1 << 4;
    pub const GUILD_WEBHOOKS: u32 = 1 << 5;
    pub const GUILD_INVITES: u32 = 1 << 6;
    pub const GUILD_VOICE_STATES: u32 = 1 << 7;
    pub const GUILD_PRESENCES: u32 = 1 << 8;
    pub const GUILD_MESSAGES: u32 = 1 << 9;
    pub const GUILD_MESSAGE_REACTIONS: u32 = 1 << 10;
    pub const GUILD_MESSAGE_TYPING: u32 = 1 << 11;
    pub const DIRECT_MESSAGES: u32 = 1 << 12;
    pub const DIRECT_MESSAGE_REACTIONS: u32 = 1 << 13;
    pub const DIRECT_MESSAGE_TYPING: u32 = 1 << 14;
    pub const MESSAGE_CONTENT: u32 = 1 << 15;

    /// Non-privileged baseline: renders servers, channels, chat, reactions,
    /// and typing without any privileged intent. Works for every bot.
    pub const BASELINE: u32 = GUILDS
        | GUILD_VOICE_STATES
        | GUILD_MESSAGES
        | GUILD_MESSAGE_REACTIONS
        | GUILD_MESSAGE_TYPING
        | DIRECT_MESSAGES
        | DIRECT_MESSAGE_REACTIONS
        | DIRECT_MESSAGE_TYPING;

    /// The full set we ask for first: baseline + members (member list) +
    /// presences (status dots) + message content (non-bot messages).
    /// If the application has these intents enabled we get the rich UX;
    /// otherwise Discord closes with 4014 and we retry with BASELINE.
    pub const FULL: u32 = BASELINE | GUILD_MEMBERS | GUILD_PRESENCES | MESSAGE_CONTENT;

    /// Kept for compatibility with older code paths.
    pub const DEFAULT: u32 = BASELINE | MESSAGE_CONTENT;
}

/// Capabilities bitmask we send in IDENTIFY. The user mentioned "16381"
/// (matches current Discord desktop client). Verified by research §7 of
/// gateway-spec.md.
pub const CLIENT_CAPABILITIES: u32 = 16381;


