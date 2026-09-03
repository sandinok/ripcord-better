//! Typed REST endpoints + body shapes for Discord v10.
//!
//! `Route::path()` returns the bucket-key form of the path — the path
//! *without* query parameters — so identical bucket-key paths hash to
//! the same `Route` value (e.g. channels/123/messages?limit=50 vs
//! channels/123/messages?limit=100 share the bucket).

use crate::model::Snowflake;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    CurrentUser,
    MyGuilds,
    Guild(Snowflake),
    GuildChannels(Snowflake),
    GuildEmojis(Snowflake),
    Channel(Snowflake),
    ChannelMessages(Snowflake),
    ChannelTyping(Snowflake),
    ChannelReactions(Snowflake),
    ChannelPins(Snowflake),
    ChannelThreads(Snowflake),
    User(Snowflake),
}

impl Route {
    pub fn path(&self) -> String {
        match self {
            Self::CurrentUser => "/users/@me".into(),
            Self::MyGuilds => "/users/@me/guilds".into(),
            Self::Guild(id) => format!("/guilds/{id}"),
            Self::GuildChannels(id) => format!("/guilds/{id}/channels"),
            Self::GuildEmojis(id) => format!("/guilds/{id}/emojis"),
            Self::Channel(id) => format!("/channels/{id}"),
            Self::ChannelMessages(id) => format!("/channels/{id}/messages"),
            Self::ChannelTyping(id) => format!("/channels/{id}/typing"),
            Self::ChannelReactions(id) => format!("/channels/{id}/messages/%/reactions"),
            Self::ChannelPins(id) => format!("/channels/{id}/pins"),
            Self::ChannelThreads(id) => format!("/channels/{id}/threads"),
            Self::User(id) => format!("/users/{id}"),
        }
    }
}

/// POST /channels/{id}/messages body.
///
/// The attachments array goes inside the JSON; the binaries are sent as
/// separate multipart parts named `files[n]`. (See research/rest-spec.md §4
/// for the multipart boundary layout.)
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CreateMessageBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "nonce")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub embeds: Vec<serde_json::Value>,
    /// Per-attachment metadata: filename, description (alt text).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub attachments: Vec<AttachmentMeta>,
    /// Reply reference — set `message_id` to convert into a reply.
    #[serde(skip_serializing_if = "Option::is_none", rename = "message_reference")]
    pub message_reference: Option<crate::model::MessageReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<u32>,
    /// Allowed mentions: parse `["users","roles","everyone"]` to restrict.
    #[serde(skip_serializing_if = "Option::is_none", rename = "allowed_mentions")]
    pub allowed_mentions: Option<AllowedMentions>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AllowedMentions {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub parse: Vec<String>, // "users" "roles" "everyone"
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub users: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub replied_user: Option<bool>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AttachmentMeta {
    pub id: u32,
    pub filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
