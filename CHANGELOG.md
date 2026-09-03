# Changelog

All notable changes to Basalt are documented here. The format follows
Keep a Changelog 1.1.0.

## 0.1.1 - 2026-09-03

The safety and completeness release. One Enter sends exactly one message,
your own messages appear instantly, and the client presents a consistent,
browser-shaped identity to Discord so sessions read as normal usage.

### Fixed

- Double sends are structurally impossible now. The composer consumes the
  Enter key event (exactly-once semantics), sends go through a single
  worker queue (one POST per message, in order), and every message carries
  a nonce that dedupes the optimistic copy against the REST response and
  the gateway MESSAGE_CREATE event. An automated test presses Enter once
  and asserts, via the API, that exactly one message exists on the server.
- No request is ever resent automatically. A failed send removes the
  optimistic copy, restores the draft into the composer and shows the
  error inline ("your message was not sent"). Retrying is a human
  decision, never a client decision.
- 429 responses now wait the full Retry-After (body JSON first, header
  fallback), mark the bucket exhausted for concurrent requests, and
  surface the failure to the UI instead of silently retrying.
- Gateway reconnects use exponential backoff (1s to 60s) with full jitter,
  reset after a connection survives 30 seconds. No more reconnect storms.
- A nonce arriving as a JSON number (how the official client sends it) no
  longer fails MESSAGE_CREATE parsing, which used to drop those messages
  from the chat.
- The settings modal stays open: the click that opens it no longer
  immediately counts as a backdrop click that closes it.
- The reaction picker no longer flash-closes on the click that opened it
  (same one-frame-click class of bug as the settings modal).
- Messages you send appear instantly: an optimistic copy renders on Enter
  and is replaced by the real message when the first delivery arrives.
- Selecting a channel or DM returns keyboard focus to the composer, so
  typing right after a click lands in the message box (it used to go
  nowhere after clicking a DM row).
- Bot accounts keep their DM list across restarts: Basalt remembers DMs
  it has seen and re-fetches them at startup (the API gives bots no DM
  list).

### Added

- Emoji reaction picker: hover a message, click the smiley, get a Discord
  style popup with a search box and a grid of 130 color Twemoji; the
  chosen reaction is sent and renders live.
- Guild banner at the top of the channel sidebar with a gradient fade,
  like the official client (when the server has a banner).
- Embed images: link unfurls render their thumbnail (small, top-right,
  like Discord) and their main image below the description. Titles are
  clickable and open in the browser.
- The client identity is consistent end to end: the HTTP User-Agent, the
  gateway IDENTIFY properties and the websocket handshake all describe
  the same Chromium-on-your-OS client, with a current build number and a
  per-launch id (modeled after the reference open-source clients). Bot
  sessions keep the documented bot User-Agent and honest properties.
- The websocket handshake sends Origin and a matching User-Agent, like a
  browser.
- New logo render with depth (top-lit center column, ground shadow) and
  embedded icons: the Windows exe carries a multi-size .ico (Explorer,
  taskbar, window chrome) and the macOS download is a proper .app bundle
  with an .icns.
- Server icons are rounder at rest (r22 of 48) and still morph to full
  circles on hover/selection.

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
