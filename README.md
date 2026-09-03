# Basalt

<img src="assets/icon-128.png" width="96" align="right" alt="Basalt logo"/>

**A native Discord client written in Rust.** No Electron, no WebView, no
Chromium in disguise. One binary, ~30 MB of RAM at idle, and a UI built
pixel by pixel with [egui](https://github.com/emilk/egui).

Basalt is named after the rock: dark, dense, and made of columns.

## What works

- Sign in with a bot token (user tokens also work, see the disclaimer below)
- Server list with real icons, hover morphing, and the white selection pill
- Channel tree with categories, emoji-colored channel names, unread badges
- Chat with markdown, code blocks, mentions, links, spoilers, and embeds
- Color emoji everywhere (Twemoji), rendered as images, crisp at any size
- Replies with a reference bar, reactions with counters
- Direct messages: the Home view lists your DMs with live avatars
- Per-user presence dots (online / idle / do not disturb / invisible) when
  the Server Members and Presence intents are enabled on your bot
- Your own status picker, right on your avatar in the user box
- Live typing indicators and new messages arriving in real time
- Settings that actually do things: font size, cozy/compact density,
  member list toggle, unread badges, mention count in the window title
- Images: avatars and attachments load from the CDN and cache in memory
- Reconnects automatically, resumes the session, and survives zombie
  heartbeats

## Install

Grab a binary from the [releases](https://github.com/sandinok/basalt/releases)
page: Windows, macOS, and Linux x86_64.

You will need a Discord token. The easiest honest option is a bot token
from the [Developer Portal](https://discord.com/developers/applications).
Create an application, add a bot, copy the token, paste it into Basalt.
Invite the bot to a server you own and you can chat immediately.

Basalt tries the full set of gateway intents first (members, presences,
message content). If your bot does not have those privileged intents
enabled, it automatically falls back to a reduced set so the client still
works: you get chat, but presence dots and member lists will be limited.

## Build from source

```sh
cargo build --release
./target/release/basalt --token <your-token>
```

Rust 1.98 or newer. On Linux you need the usual X11 or Wayland client
libraries; everything else is bundled through crates.io.

## Why

Electron Discord uses about 300 to 800 MB of RAM. Basalt idles around
30 MB while speaking the same protocol. It is also a research vehicle:
how far can a from-scratch client get before the Discord API fights back?

## Disclaimer

Basalt is not affiliated with Discord Inc. Automating user accounts
violates Discord's Terms of Service; bot tokens are the supported path.

## License

MIT. Icon geometry from Google's Material Symbols (Apache 2.0). Emoji
art from Twemoji (jdecked fork, CC-BY 4.0).
