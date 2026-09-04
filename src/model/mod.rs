//! Discord data types. Maps to REST v10 + Gateway v10 payloads.
//!
//! Source-of-truth for field shapes: research/rest-spec.md and
//! research/gateway-spec.md (generated 2026-08-30). Field naming follows
//! the Discord docs (`global_name`, `public_flags`, etc.) — NOT web.js
//! camelCase so we can spot upstream renames.

pub mod snowflake;

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub use snowflake::Snowflake;

// ───────────────────────────── Users ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct User {
    pub id: Snowflake,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub discriminator: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub bot: bool,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub public_flags: Option<u64>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub accent_color: Option<u32>,
    /// Locale (BCP-47) — only present on `users/@me`.
    #[serde(default)]
    pub locale: Option<String>,
    /// Email — only present on `users/@me` for non-bot accounts.
    #[serde(default)]
    pub email: Option<String>,
    /// Bitfield of `UserFlags`. Verified 2026: 1<<23 STAFF, 1<<19
    /// CERTIFIED_MODERATOR, 1<<22 ACTIVE_DEVELOPER etc.
    #[serde(default)]
    pub flags: Option<u64>,
    #[serde(default)]
    pub mfa_enabled: Option<bool>,
    #[serde(default)]
    pub premium_type: Option<u8>,
}

impl User {
    /// Display name preferred order: global_name → username → "Unknown".
    pub fn display_name(&self) -> &str {
        self.global_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(Some(self.username.as_str()))
            .filter(|s| !s.is_empty())
            .unwrap_or("Unknown")
    }

    /// Avatar URL on cdn.discordapp.com. `size=256` is a good balance of
    /// quality vs. weight for a 40-px avatar (~10 KiB WebP). Falls back to
    /// a default avatar (deterministic by user ID mod 6).
    pub fn avatar_url(&self) -> String {
        let ext = if self
            .avatar
            .as_deref()
            .map(|h| h.starts_with("a_"))
            .unwrap_or(false)
        {
            "gif"
        } else {
            "webp"
        };
        if let Some(h) = &self.avatar {
            return format!("https://cdn.discordapp.com/avatars/{}/{}.{}?size=256", self.id, h, ext);
        }
        // Default avatars are NUMBERED 0-5 (`embed/avatars/{n}.png`), keyed
        // by (user_id >> 22) % 6 - the same mapping the official client uses.
        // The old color-name URLs (embed/avatars/purple.png) 404'd, which is
        // why avatarless users rendered as broken placeholders.
        let bucket = (u64::from(self.id) >> 22) % 6;
        format!("https://cdn.discordapp.com/embed/avatars/{}.png?size=128", bucket)
    }

    /// Profile banner URL (user-level banner), when the user has one.
    pub fn banner_url(&self) -> Option<String> {
        let h = self.banner.as_deref()?;
        let ext = if h.starts_with("a_") { "gif" } else { "webp" };
        Some(format!("https://cdn.discordapp.com/banners/{}/{}.{}?size=600", self.id, h, ext))
    }

    /// Accent color as an egui color (for banners we render as gradient).
    pub fn accent(&self) -> Option<egui::Color32> {
        self.accent_color.map(|c| egui::Color32::from_rgb(
            ((c >> 16) & 0xFF) as u8,
            ((c >> 8) & 0xFF) as u8,
            (c & 0xFF) as u8,
        ))
    }
}

// ───────────────────────────── Guilds ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Guild {
    pub id: Snowflake,
    /// READY delivers guild stubs ({id, unavailable}) without a name;
    /// GUILD_CREATE fills it in. Default to empty and let the UI show a
    /// placeholder until then.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub owner: Option<bool>,
    #[serde(default)]
    pub owner_id: Option<Snowflake>,
    #[serde(default)]
    pub permissions: Option<String>, // stringified u64
    #[serde(default)]
    pub premium_tier: Option<u8>,
    #[serde(default)]
    pub premium_subscription_count: Option<u32>,
    #[serde(default)]
    pub vanity_url_code: Option<String>,
    #[serde(default)]
    pub member_count: Option<u32>,
    /// Approximate member count (only present with ?with_counts=true).
    #[serde(default)]
    pub approximate_member_count: Option<u32>,
    #[serde(default)]
    pub approximate_presence_count: Option<u32>,
    /// Latest role icon (if shipped). Real roles come from GUILD_CREATE payload.
    #[serde(default)]
    pub roles: Vec<Role>,
    #[serde(default)]
    pub emojis: Vec<Emoji>,
    #[serde(default)]
    pub stickers: Vec<StickerItem>,
    /// Channels - only present in GUILD_CREATE (full payload), not in
    /// READY's guild stubs or REST guild list responses.
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// Members - GUILD_CREATE with the GUILD_MEMBERS intent delivers the
    /// initial chunk (large guilds stream in via GUILD_MEMBERS_CHUNK).
    #[serde(default)]
    pub members: Vec<Member>,
    /// Presences - GUILD_CREATE with the GUILD_PRESENCES intent.
    #[serde(default)]
    pub presences: Vec<PresenceUpdate>,
    /// True while the full payload hasn't arrived yet (READY stubs).
    #[serde(default)]
    pub unavailable: bool,
}

impl Guild {
    pub fn icon_url(&self) -> Option<String> {
        let h = self.icon.as_ref()?;
        // Animated guild icons use an `a_` prefix, same as avatars.
        let ext = if h.starts_with("a_") { "gif" } else { "png" };
        Some(format!("https://cdn.discordapp.com/icons/{}/{}.{}?size=128", self.id, h, ext))
    }

    /// Guild banner (wide strip) for the top of the channel sidebar, as the
    /// official client shows it.
    pub fn banner_url(&self) -> Option<String> {
        let h = self.banner.as_ref()?;
        Some(format!("https://cdn.discordapp.com/banners/{}/{}.png?size=600", self.id, h))
    }

    pub fn initials(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            return "?".to_string();
        }
        let initials: String = name
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase();
        if initials.is_empty() {
            name.chars().take(2).collect::<String>().to_uppercase()
        } else {
            initials
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Role {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub color: u32,
    #[serde(default)]
    pub hoist: bool,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub unicode_emoji: Option<String>,
    #[serde(default)]
    pub position: i32,
    #[serde(default)]
    pub permissions: String,
    #[serde(default)]
    pub mentionable: bool,
}

// ───────────────────────────── Channels ─────────────────────────────

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq)]
pub enum ChannelType {
    #[default]
    Text = 0,
    Dm = 1,
    Voice = 2,
    GroupDm = 3,
    Category = 4,
    News = 5,
    NewsThread = 10,
    PublicThread = 11,
    PrivateThread = 12,
    StageVoice = 13,
    Directory = 14,
    Forum = 15,
    Media = 16,
}

impl<'de> Deserialize<'de> for ChannelType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = u8::deserialize(d)?;
        Ok(match n {
            0 => Self::Text,
            1 => Self::Dm,
            2 => Self::Voice,
            3 => Self::GroupDm,
            4 => Self::Category,
            5 => Self::News,
            10 => Self::NewsThread,
            11 => Self::PublicThread,
            12 => Self::PrivateThread,
            13 => Self::StageVoice,
            14 => Self::Directory,
            15 => Self::Forum,
            16 => Self::Media,
            _ => Self::Text,
        })
    }
}

impl Serialize for ChannelType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(*self as u8)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Channel {
    pub id: Snowflake,
    #[serde(default, rename = "type")]
    pub kind: ChannelType,
    /// DM channels may carry `null`; display_name() handles the fallback.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    #[serde(default)]
    pub parent_id: Option<Snowflake>,
    #[serde(default)]
    pub position: Option<i32>,
    /// Last message ID (snowflake) — used to sort channels by recency.
    #[serde(default)]
    pub last_message_id: Option<Snowflake>,
    /// For voice channels.
    #[serde(default)]
    pub bitrate: Option<u32>,
    #[serde(default)]
    pub user_limit: Option<u32>,
    /// Recipients array — present only on DM / GroupDM channels.
    #[serde(default)]
    pub recipients: Vec<User>,
    /// Recipient IDs — present only on `private_channels_v2` from READY.
    #[serde(default)]
    pub recipient_ids: Vec<Snowflake>,
    #[serde(default)]
    pub nsfw: bool,
    /// For Forum: available_tags (deprecated; we don't render these in v0).
    #[serde(default)]
    pub flags: Option<u32>,
}

impl Channel {
    pub fn display_name(&self) -> String {
        match self.kind {
            ChannelType::Dm => {
                self.recipients
                    .first()
                    .map(|u| u.display_name().to_string())
                    .unwrap_or_else(|| "Unknown".into())
            }
            ChannelType::GroupDm => {
                let names: Vec<&str> = self.recipients.iter().take(3).map(|u| u.display_name()).collect();
                if names.is_empty() {
                    "Empty Group".into()
                } else if self.recipients.len() > 3 {
                    format!("{} +{}", names.join(", "), self.recipients.len() - 3)
                } else {
                    names.join(", ")
                }
            }
            _ => {
                if self.name.is_empty() {
                    "empty-channel".into()
                } else {
                    self.name.clone()
                }
            }
        }
    }
    pub fn is_text_like(&self) -> bool {
        matches!(
            self.kind,
            ChannelType::Text | ChannelType::Dm | ChannelType::GroupDm | ChannelType::News
                | ChannelType::NewsThread | ChannelType::PublicThread | ChannelType::PrivateThread
        )
    }
    pub fn is_voice_like(&self) -> bool {
        matches!(
            self.kind,
            ChannelType::Voice | ChannelType::StageVoice
        )
    }
    /// Recipient user for a DM channel, resolved against a user lookup.
    pub fn dm_recipient(&self, fallback: impl Fn(Snowflake) -> Option<User>) -> Option<User> {
        if let Some(u) = self.recipients.first() {
            return Some(u.clone());
        }
        self.recipient_ids.first().and_then(|&id| fallback(id))
    }
}

// ───────────────────────────── Messages ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Message {
    pub id: Snowflake,
    pub channel_id: Snowflake,
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    pub author: User,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub mentions: Vec<User>,
    #[serde(default)]
    pub mention_roles: Vec<Snowflake>,
    #[serde(default)]
    pub mention_everyone: bool,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub kind: u8, // message type 0=DEFAULT 19=REPLY 20=THREAD_STARTER etc.
    #[serde(default)]
    pub flags: u32,
    /// Reply reference (for type 19 = REPLY). Contains the referenced message id.
    #[serde(default)]
    pub message_reference: Option<MessageReference>,
    /// Stickers attached to this message.
    #[serde(default)]
    pub sticker_items: Vec<StickerItem>,
    #[serde(default)]
    pub referenced_message: Option<Box<Message>>,
    /// Client nonce echoed back on MESSAGE_CREATE. Discord sends this as
    /// either a string OR a raw integer depending on the sender's client,
    /// so the deserializer accepts both (a numeric nonce from the official
    /// client used to fail the whole Message parse and drop the message).
    #[serde(default, deserialize_with = "deserialize_nonce")]
    pub nonce: Option<String>,
    /// Components (buttons / selects) — only render text in v0.
    #[serde(default)]
    pub components: Vec<serde_json::Value>,
}

/// `nonce` arrives as a JSON string or integer; normalize to String.
fn deserialize_nonce<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde_json::Value;
    let v = Option::<Value>::deserialize(d)?;
    Ok(match v {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

impl Message {
    pub fn timestamp_dt(&self) -> Option<OffsetDateTime> {
        let s = self.timestamp.as_ref()?;
        parse_iso(s)
    }
    pub fn edited_dt(&self) -> Option<OffsetDateTime> {
        let s = self.edited_timestamp.as_ref()?;
        if s.is_empty() {
            return None;
        }
        parse_iso(s)
    }
}

fn parse_iso(s: &str) -> Option<OffsetDateTime> {
    // Discord uses ISO 8601 like 2026-08-30T12:34:56.789000+00:00
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_display_name_prefers_global_name() {
        let mut u = User::default();
        u.username = "user123".into();
        u.global_name = Some("Alice".into());
        assert_eq!(u.display_name(), "Alice");
    }

    #[test]
    fn user_display_name_falls_back_to_username() {
        let mut u = User::default();
        u.username = "user123".into();
        u.global_name = None;
        assert_eq!(u.display_name(), "user123");
    }

    #[test]
    fn user_display_name_handles_empty_global_name() {
        let mut u = User::default();
        u.username = "user123".into();
        u.global_name = Some(String::new()); // empty
        assert_eq!(u.display_name(), "user123");
    }

    #[test]
    fn user_display_name_handles_both_empty() {
        let u = User::default();
        assert_eq!(u.display_name(), "Unknown");
    }

    #[test]
    fn user_avatar_url_for_animated_avatar() {
        let mut u = User::default();
        u.id = Snowflake::from_u64(1);
        u.avatar = Some("a_abcdef".into());
        let url = u.avatar_url();
        assert!(url.contains(".gif?"), "animated avatars should use .gif, got {url}");
        assert!(url.contains("/avatars/1/"));
    }

    #[test]
    fn user_avatar_url_for_static_avatar() {
        let mut u = User::default();
        u.id = Snowflake::from_u64(42);
        u.avatar = Some("abcdef0123456789".into());
        let url = u.avatar_url();
        assert!(url.contains(".webp?"), "static avatars should use .webp, got {url}");
    }

    #[test]
    fn user_avatar_url_for_default_avatar_uses_embed() {
        let mut u = User::default();
        // (7 >> 22) % 6 == 0 → numbered default avatar 0.
        u.id = Snowflake::from_u64(7);
        u.avatar = None;
        let url = u.avatar_url();
        assert!(url.contains("/embed/avatars/"), "missing avatar should fall back to embed, got {url}");
        assert!(url.contains("avatars/0."), "(7>>22)%6 == 0 must map to avatar 0, got {url}");
    }

    #[test]
    fn default_avatar_bucket_is_id_shifted() {
        // The official mapping is (user_id >> 22) % 6, NOT id % 6.
        let mut u = User::default();
        u.avatar = None;
        u.id = Snowflake::from_u64(1 << 22); // (1<<22 >> 22) % 6 = 1
        assert!(u.avatar_url().contains("avatars/1."));
        u.id = Snowflake::from_u64(2 << 22 | 5); // (2<<22|5 >> 22) % 6 = 2
        assert!(u.avatar_url().contains("avatars/2."));
    }

    #[test]
    fn guild_initials_single_word() {
        let mut g = Guild::default();
        g.name = "Discord".into();
        assert_eq!(g.initials(), "D");
    }

    #[test]
    fn guild_initials_two_words() {
        let mut g = Guild::default();
        g.name = "Foo Bar".into();
        assert_eq!(g.initials(), "FB");
    }

    #[test]
    fn guild_initials_empty_name() {
        let g = Guild::default();
        assert_eq!(g.initials(), "?");
    }

    #[test]
    fn guild_icon_url_for_animated_icon() {
        let mut g = Guild::default();
        g.id = Snowflake::from_u64(100);
        // Animated guild icons have the `a_` prefix, same as avatars.
        g.icon = Some("a_1234567890abcdef".into());
        let url = g.icon_url().unwrap();
        assert!(url.contains(".gif"), "animated guild icons should use .gif, got {url}");
    }

    #[test]
    fn guild_icon_url_for_static_icon() {
        let mut g = Guild::default();
        g.id = Snowflake::from_u64(101);
        g.icon = Some("abcdef0123456789".into()); // no a_ prefix
        let url = g.icon_url().unwrap();
        assert!(url.contains(".png"), "static guild icons should use .png, got {url}");
    }

    #[test]
    fn channel_display_name_for_text_uses_name() {
        let mut c = Channel::default();
        c.name = "general".into();
        c.kind = ChannelType::Text;
        assert_eq!(c.display_name(), "general");
    }

    #[test]
    fn channel_display_name_empty_text_shows_placeholder() {
        let mut c = Channel::default();
        c.name = "".into();
        c.kind = ChannelType::Text;
        assert_eq!(c.display_name(), "empty-channel");
    }

    #[test]
    fn channel_display_name_dm_uses_recipient_name() {
        let mut c = Channel::default();
        c.kind = ChannelType::Dm;
        let mut u = User::default();
        u.username = "bob".into();
        u.global_name = Some("Bob".into());
        c.recipients.push(u);
        assert_eq!(c.display_name(), "Bob");
    }

    #[test]
    fn reaction_emoji_content_string_custom_static() {
        let mut r = ReactionEmoji::default();
        r.id = Some(Snowflake::from_u64(123));
        r.name = Some("smile".into());
        assert_eq!(r.content_string(), "<:smile:123>");
    }

    #[test]
    fn reaction_emoji_content_string_custom_animated() {
        let mut r = ReactionEmoji::default();
        r.id = Some(Snowflake::from_u64(456));
        r.name = Some("dance".into());
        r.animated = true;
        assert_eq!(r.content_string(), "<a:dance:456>");
    }

    #[test]
    fn reaction_emoji_content_string_unicode() {
        let mut r = ReactionEmoji::default();
        r.name = Some("🔥".into());
        assert_eq!(r.content_string(), "🔥");
    }

    #[test]
    fn message_timestamp_parses_iso8601() {
        let mut m = Message::default();
        m.timestamp = Some("2026-08-30T12:34:56.789000+00:00".into());
        let dt = m.timestamp_dt().expect("should parse");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), time::Month::August);
        assert_eq!(dt.day(), 30);
    }

    #[test]
    fn message_timestamp_handles_empty() {
        let mut m = Message::default();
        m.timestamp = Some(String::new());
        assert!(m.timestamp_dt().is_none());
    }

    #[test]
    fn message_timestamp_handles_garbage() {
        let mut m = Message::default();
        m.timestamp = Some("not a date".into());
        assert!(m.timestamp_dt().is_none());
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Attachment {
    pub id: Snowflake,
    pub filename: String,
    pub size: u64,
    pub url: String,
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Embed {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub footer: Option<EmbedFooter>,
    #[serde(default)]
    pub image: Option<EmbedImage>,
    #[serde(default)]
    pub thumbnail: Option<EmbedImage>,
    #[serde(default)]
    pub author: Option<EmbedAuthor>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbedFooter {
    pub text: String,
    #[serde(default)]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbedImage {
    pub url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbedAuthor {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reaction {
    pub count: u32,
    pub me: bool,
    /// For custom emoji: { id, name, animated }. For unicode: { name: "🔥" }.
    pub emoji: ReactionEmoji,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReactionEmoji {
    #[serde(default)]
    pub id: Option<Snowflake>,
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
}

impl ReactionEmoji {
    /// Renders as the Discord content string (`🔥`, `<:name:id>`, `<a:name:id>`).
    pub fn content_string(&self) -> String {
        match (&self.id, &self.name) {
            (Some(id), Some(name)) if self.animated => format!("<a:{name}:{id}>"),
            (Some(id), Some(name)) => format!("<:{name}:{id}>"),
            (_, Some(name)) => name.clone(),
            _ => String::new(),
        }
    }

    /// CDN URL for a custom (guild) emoji: animated ones are GIFs, static
    /// ones WebP. Used to render reactions and emoji in message content.
    pub fn custom_emoji_url(&self) -> Option<String> {
        let id = self.id?;
        let ext = if self.animated { "gif" } else { "webp" };
        Some(format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=64&quality=lossless"))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageReference {
    pub message_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    pub guild_id: Option<Snowflake>,
}

// ───────────────────────────── Presence / Member ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub nick: Option<String>,
    #[serde(default)]
    pub roles: Vec<Snowflake>,
    #[serde(default)]
    pub joined_at: Option<String>,
    #[serde(default)]
    pub premium_since: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub deaf: bool,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub communication_disabled_until: Option<String>,
}

/// The partial user object that rides along PRESENCE_UPDATE and
/// GUILD_CREATE.presences - Discord only guarantees `id` there.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartialUser {
    pub id: Snowflake,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub discriminator: String,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub bot: bool,
}

impl PartialUser {
    /// Full-enough display name, if the payload carried one.
    pub fn display_name(&self) -> Option<&str> {
        self.global_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.username.as_deref())
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PresenceUpdate {
    #[serde(default)]
    pub user: PartialUser,
    pub status: String, // "online" | "idle" | "dnd" | "offline"
    #[serde(default)]
    pub activities: Vec<Activity>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Activity {
    #[serde(rename = "type")]
    pub kind: u8, // 0=Game 1=Streaming 2=Listening 3=Custom 5=Competing
    pub name: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub details: Option<String>,
    #[serde(default)]
    pub application_id: Option<Snowflake>,
    #[serde(default)]
    pub timestamps: Option<BTreeMap<String, u64>>,
    #[serde(default)]
    pub emoji: Option<ReactionEmoji>,
    #[serde(default)]
    pub assets: Option<BTreeMap<String, String>>,
}

// ───────────────────────────── Voice state ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceState {
    pub user_id: Snowflake,
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    pub channel_id: Option<Snowflake>,
    #[serde(default)]
    pub self_mute: bool,
    #[serde(default)]
    pub self_deaf: bool,
    #[serde(default)]
    pub suppress: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TypingStart {
    pub channel_id: Snowflake,
    pub user_id: Snowflake,
    pub timestamp: u64, // unix epoch seconds
    #[serde(default)]
    pub guild_id: Option<Snowflake>,
    #[serde(default)]
    pub member: Option<Member>,
}

// ───────────────────────────── v0.2 additions ─────────────────────────────

/// A custom guild emoji.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Emoji {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub animated: bool,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub available: bool,
}

impl Emoji {
    /// CDN URL for this emoji.
    pub fn url(&self) -> String {
        let ext = if self.animated { "gif" } else { "png" };
        format!("https://cdn.discordapp.com/emojis/{}.{}", self.id, ext)
    }
    /// The `<:name:id>` / `<a:name:id>` mention form used in message content.
    pub fn mention(&self) -> String {
        if self.animated {
            format!("<a:{}:{}>", self.name, self.id)
        } else {
            format!("<:{}:{}>", self.name, self.id)
        }
    }
}

/// YouTube oEmbed metadata (no API key required).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OEmbedInfo {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

/// A sticker attached to a message (`sticker_items`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StickerItem {
    pub id: Snowflake,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 1 = PNG, 2 = APNG, 3 = Lottie, 4 = GIF.
    #[serde(default)]
    pub format_type: u8,
}

impl StickerItem {
    pub fn url(&self) -> String {
        let ext = match self.format_type {
            3 => "json",
            4 => "gif",
            _ => "png",
        };
        format!("https://cdn.discordapp.com/stickers/{}.{}", self.id, ext)
    }
}

/// User profile extras (bio, pronouns) from GET /users/{id}/profile.
/// Bot tokens can read this for other bots and for members of their
/// guilds; when Discord refuses we degrade to the plain user object.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserProfile {
    #[serde(default)]
    pub bio: Option<String>,
    #[serde(default)]
    pub pronouns: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub accent_color: Option<u32>,
    #[serde(default)]
    pub premium_type: Option<u8>,
    #[serde(default)]
    pub user: Option<User>,
}

/// A guild scheduled event (sidebar Events popup).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduledEvent {
    pub id: Snowflake,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: u8,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub user_count: Option<u32>,
}
