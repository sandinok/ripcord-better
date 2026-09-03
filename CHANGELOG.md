# Changelog

All notable changes to Basalt are documented here. The format follows
Keep a Changelog 1.1.0.

## 0.1.0 - 2026-09-03

The first Basalt release: a new name, a new logo, and a UI that finally
looks like a real chat client.

### Added

- Full rebrand: Basalt name, logo (three hexagonal stone columns), window
  icon, user agent, and config path under ~/.config/basalt/.
- Real icon system: 55 Google Material Symbols embedded as authentic SVG
  path data, parsed and rasterized at runtime with a nonzero-winding
  scanline renderer and 4x supersampling. Crisp at any DPI.
- Color emoji everywhere (Twemoji via CDN with offline caching) in channel
  names, messages, reactions, and the composer area. Full cluster
  segmentation: ZWJ sequences, skin tones, keycaps, and flags.
- Home view with the DM list, live avatars, status dots, and a live
  conversation filter.
- Per-channel message caches: switching channels is instant, and gateway
  events update the right channel instead of polluting the current view.
- Presence dots on avatars (online, idle, do not disturb, invisible) and
  your own status picker in the user box, sent through the gateway.
- Member list panel with REST fallback when the members intent is missing.
- Unread badges and mention counters on channels and servers, plus a
  mention count in the window title (both can be turned off).
- Typing indicators above the composer.
- Replies: hover a message, hit reply, and the composer sends with the
  message reference. Reply bars render on both the composer and messages.
- Reactions with color emoji and live add/remove counts.
- Settings modal with four sections that all work: My Account (status,
  sign out), Appearance (font size with live preview, density, member
  list), Notifications (badges, title counter), and About.
- Discord-style server bar: rounded icons that morph into circles, a white
  selection pill that animates, tooltips, and unread badges.
- One unified empty state (no more contradictory "No channel" plus
  "start of the channel" messages) and a disabled-but-visible composer.
- Automatic token type detection (bot vs user) and bot-shaped IDENTIFY,
  which fixes invalid-session loops on bot accounts.
- Graceful intent downgrade: if the gateway rejects privileged intents,
  Basalt reconnects with a baseline set and keeps working.
- Window title, taskbar icon, and login screen all use the new Basalt
  brand drawn as vectors.

### Fixed

- The chat panel now fills the entire window; the black void below the
  chat area is gone (the layout uses real egui panels now).
- Messages load when you enter a channel, and the composer is always
  visible with Enter-to-send.
- Messages render with dynamic row heights, so long messages no longer
  clip (the old 64px fixed rows are gone).
- The gateway zlib-stream decoder now keeps one persistent inflate
  context (context takeover) and treats sync-flush markers correctly;
  before, the second frame of every session failed to decode.
- Heartbeat ACK tracking reads the shared state instead of a dead local
  variable, so the client no longer reconnects every 41 seconds.
- Gateway reconnection flow: invalid-session and server-requested
  reconnect events now actually reconnect with the right strategy
  (resume vs fresh identify).
- READY and GUILD_CREATE payloads parse against real Discord data:
  session_type strings, partial presence users, and name-less guild
  stubs no longer abort the session.
- DM channels with a null name decode correctly, so bots can discover
  their DMs from incoming messages (Discord does not give bots a DM list).
- REST message history is reversed into chronological order; the cache
  merge logic no longer discards history when a live event arrives first.
- Settings modal closes properly from the X button, Escape, and the
  backdrop click (the shared open flag was fighting the local one).
- The sidebar and server bar reserve space for their pinned bottom rows,
  so the user box and the settings gear are no longer clipped off-screen.

### Changed

- New version numbering starting at 0.1.0.
- 83 unit tests covering the icon parser and rasterizer, emoji
  segmentation, markdown, models, the zlib gateway decoder with context
  takeover, per-channel caches, unread tracking, and reactions.
- Clippy clean with -D warnings.

### Known limitations

- Voice channels are listed but not joinable; voice arrives in a future
  release.
- File uploads are not implemented yet.
- Presence dots for other users require the privileged intents to be
  enabled on your bot application.
