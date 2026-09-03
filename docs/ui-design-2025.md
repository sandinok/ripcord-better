# UI design tokens (2026)

Hex values verified against Discord's live `web.js` design-token bundle on
2026-08-30. The user-supplied palette had 9 of 12 exact matches; three
were corrected and are noted below.

## Backgrounds (PRIMARY scale)

| Token | Hex | Use |
|---|---|---|
| PRIMARY_700 | `#1E1F22` | guilds bar |
| PRIMARY_660 | `#232428` | secondary alt (spoiler revealed) |
| PRIMARY_630 | `#2B2D31` | channels sidebar, members, embed bg |
| PRIMARY_600 | `#313338` | chat |
| PRIMARY_560 | `#383A40` | inputs |
| PRIMARY_530 | `#41424A` | accent / hover bg |
| PRIMARY_730 | `#1A1B1E` | floating windows |

## Brand

| Token | Hex |
|---|---|
| BRAND_500 (Blurple) | `#5865F2` |
| BRAND_560 | `#4752C4` |
| BRAND_600 | `#3C45A5` |

## Text

| Token | Hex |
|---|---|
| WHITE_500 | `#FFFFFF` |
| HEADER_PRIMARY | `#F9F9F9` |
| PRIMARY_230 | `#DBDEE1` |
| PRIMARY_330 | `#B5BAC1` |
| NEUTRAL_27 | `#9D9EA5` |
| TEXT_LINK | `#00A8FC` |

## Status dots (current)

| State | Token | Hex |
|---|---|---|
| online | GREEN_NEW_40 | `#3D9E60` |
| idle | YELLOW_NEW_22 | `#FFCB6E` |
| dnd | RED_NEW_45 | `#DC4247` |
| offline | NEUTRAL_27 | `#9D9EA5` |

## Status dots (legacy 2022-2024, available behind a pref)

| State | Hex |
|---|---|
| online | `#248046` |
| idle | `#F0B232` |
| dnd | `#ED4245` |
| offline | `#80848E` |

## Corrections to the user-supplied palette

- Mention background alpha is **0.239** (rgba(88,101,242,61)), not 0.15.
- Code-block background is a subtle **blurple tint** (rgba(88,101,242,20)),
  not a solid `#1E1F22`.
- Code-block text is `#FFFFFF`, not `#B5BAC1`.

## Squircle

Corner aperture factor is **0.464** (sampled from `svg-mask-squircle`
in `web.js`). The often-cited 22.37% figure refers to iOS app-icon corner
radius as a fraction of icon width - it is not the squircle smoothing factor.

## Fonts

In-app font is **gg sans** (Discord's proprietary variable font, since
January 2023). Whitney is deprecated. Inter is not used. We use egui's
default proportional font (`Noto Sans` family) as a substitute because
gg sans is not publicly redistributable.
