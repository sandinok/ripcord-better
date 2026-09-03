//! Discord 2025 production palette.
//!
//! Hex values verified against Discord's live `web.js` design-token
//! bundle on 2026-08-30. See docs/ui-design-2025.md for citations.

#![allow(dead_code)]

use egui::Color32;

// Backgrounds - PRIMARY scale
pub const BG_GUILDS_BAR: Color32 = Color32::from_rgb(0x1E, 0x1F, 0x22);
pub const BG_SIDEBAR: Color32 = Color32::from_rgb(0x2B, 0x2D, 0x31);
pub const BG_CHAT: Color32 = Color32::from_rgb(0x31, 0x33, 0x38);
pub const BG_INPUT: Color32 = Color32::from_rgb(0x38, 0x3A, 0x40);
pub const BG_ACCENT: Color32 = Color32::from_rgb(0x41, 0x42, 0x4A);
pub const BG_SECONDARY_ALT: Color32 = Color32::from_rgb(0x23, 0x24, 0x28);
pub const BG_FLOATING: Color32 = Color32::from_rgb(0x1A, 0x1B, 0x1E);
pub const BG_MESSAGE_HOVER: Color32 = Color32::from_black_alpha(20);

// Brand
pub const BLURPLE: Color32 = Color32::from_rgb(0x58, 0x65, 0xF2);
pub const BLURPLE_HOVER: Color32 = Color32::from_rgb(0x47, 0x52, 0xC4);
pub const BLURPLE_ACTIVE: Color32 = Color32::from_rgb(0x3C, 0x45, 0xA5);
pub const BLURPLE_SOFT: Color32 = Color32::from_rgb(0x4E, 0x5F, 0xE0);
pub const BLURPLE_TINT: Color32 = Color32::from_rgba_premultiplied(0x58, 0x65, 0xF2, 30);

pub const GREEN: Color32 = Color32::from_rgb(0x24, 0x80, 0x46);
pub const GREEN_HOVER: Color32 = Color32::from_rgb(0x1A, 0x66, 0x35);
pub const RED: Color32 = Color32::from_rgb(0xDA, 0x37, 0x3C);
pub const RED_HOVER: Color32 = Color32::from_rgb(0xA1, 0x28, 0x2D);

// Text
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const TEXT_HEADER: Color32 = Color32::from_rgb(0xF9, 0xF9, 0xF9);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xDB, 0xDE, 0xE1);
pub const TEXT_TERTIARY: Color32 = Color32::from_rgb(0xB5, 0xBA, 0xC1);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x9D, 0x9E, 0xA5);
pub const TEXT_LINK: Color32 = Color32::from_rgb(0x00, 0xA8, 0xFC);

// Status dots
pub const STATUS_ONLINE: Color32 = Color32::from_rgb(0x3D, 0x9E, 0x60);
pub const STATUS_IDLE: Color32 = Color32::from_rgb(0xFF, 0xCB, 0x6E);
pub const STATUS_DND: Color32 = Color32::from_rgb(0xDC, 0x42, 0x47);
pub const STATUS_OFFLINE: Color32 = Color32::from_rgb(0x9D, 0x9E, 0xA5);

pub const STATUS_ONLINE_LEGACY: Color32 = Color32::from_rgb(0x24, 0x80, 0x46);
pub const STATUS_IDLE_LEGACY: Color32 = Color32::from_rgb(0xF0, 0xB2, 0x32);
pub const STATUS_DND_LEGACY: Color32 = Color32::from_rgb(0xED, 0x42, 0x45);
pub const STATUS_OFFLINE_LEGACY: Color32 = Color32::from_rgb(0x80, 0x84, 0x8E);

// Special
pub const MENTION_BG: Color32 = Color32::from_rgba_premultiplied(0x58, 0x65, 0xF2, 61);
pub const MENTION_FG: Color32 = Color32::from_rgb(0xCD, 0xD7, 0xFF);
pub const CODE_BG: Color32 = Color32::from_rgba_premultiplied(0x58, 0x65, 0xF2, 20);
pub const SPOILER_HIDDEN: Color32 = Color32::from_rgb(0x7D, 0x7E, 0x87);
pub const SPOILER_REVEALED: Color32 = Color32::from_rgb(0x23, 0x24, 0x28);
pub const EMBED_BG: Color32 = Color32::from_rgb(0x2B, 0x2D, 0x31);
pub const MESSAGE_MENTION_BG: Color32 =
    Color32::from_rgba_premultiplied(0xFC, 0xB2, 0x33, 20);
pub const DEFAULT_ROLE_COLOR: Color32 = Color32::from_rgb(0xDC, 0xDD, 0xE1);

pub const RING_ONLINE: Color32 = STATUS_ONLINE;
pub const RING_IDLE: Color32 = STATUS_IDLE;
pub const RING_DND: Color32 = STATUS_DND;
pub const RING_OFFLINE: Color32 = STATUS_OFFLINE;

// The 17 default Discord role colors.
pub fn default_role_color_hex(idx: usize) -> Color32 {
    const PALETTE: [Color32; 18] = [
        Color32::from_rgb(0x1A, 0x1A, 0x1A),
        Color32::from_rgb(0x34, 0x9B, 0xD9),
        Color32::from_rgb(0x2E, 0xCC, 0x71),
        Color32::from_rgb(0xF1, 0xC4, 0x0F),
        Color32::from_rgb(0xE7, 0x3C, 0x3E),
        Color32::from_rgb(0xEA, 0x43, 0x80),
        Color32::from_rgb(0x00, 0xA8, 0xFC),
        Color32::from_rgb(0xBF, 0x37, 0xFB),
        Color32::from_rgb(0xE8, 0x6F, 0x16),
        Color32::from_rgb(0x41, 0x32, 0x9E),
        Color32::from_rgb(0x35, 0x9B, 0xB8),
        Color32::from_rgb(0xC6, 0x2E, 0x9F),
        Color32::from_rgb(0xF2, 0x8B, 0x1E),
        Color32::from_rgb(0x6D, 0x3F, 0xED),
        Color32::from_rgb(0x46, 0x35, 0x28),
        Color32::from_rgb(0x71, 0x60, 0xE4),
        Color32::from_rgb(0x6B, 0x6F, 0x77),
        Color32::from_rgb(0x00, 0xD0, 0x6B),
    ];
    PALETTE[idx % PALETTE.len()]
}

pub fn status_color(presence: &str) -> Color32 {
    match presence {
        "online" => STATUS_ONLINE,
        "idle" => STATUS_IDLE,
        "dnd" => STATUS_DND,
        _ => STATUS_OFFLINE,
    }
}

/// Linear interpolation between two colors. `t` is 0..=1.
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ar = a.r() as f32;
    let ag = a.g() as f32;
    let ab = a.b() as f32;
    let aa = a.a() as f32;
    let br = b.r() as f32;
    let bg = b.g() as f32;
    let bb = b.b() as f32;
    let ba = b.a() as f32;
    Color32::from_rgba_premultiplied(
        (ar + (br - ar) * t) as u8,
        (ag + (bg - ag) * t) as u8,
        (ab + (bb - ab) * t) as u8,
        (aa + (ba - aa) * t) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_at_zero_returns_a() {
        let a = Color32::from_rgb(10, 20, 30);
        let b = Color32::from_rgb(200, 100, 50);
        let out = lerp(a, b, 0.0);
        assert_eq!(out.r(), 10);
        assert_eq!(out.g(), 20);
        assert_eq!(out.b(), 30);
    }

    #[test]
    fn lerp_at_one_returns_b() {
        let a = Color32::from_rgb(10, 20, 30);
        let b = Color32::from_rgb(200, 100, 50);
        let out = lerp(a, b, 1.0);
        assert_eq!(out.r(), 200);
        assert_eq!(out.g(), 100);
        assert_eq!(out.b(), 50);
    }

    #[test]
    fn lerp_at_half_is_midpoint() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(100, 100, 100);
        let out = lerp(a, b, 0.5);
        assert_eq!(out.r(), 50);
        assert_eq!(out.g(), 50);
        assert_eq!(out.b(), 50);
    }

    #[test]
    fn lerp_clamps_above_one() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(100, 100, 100);
        let out = lerp(a, b, 2.0); // t > 1 should clamp to 1
        assert_eq!(out.r(), 100);
    }

    #[test]
    fn status_color_maps_known_statuses() {
        assert_eq!(status_color("online"), STATUS_ONLINE);
        assert_eq!(status_color("idle"), STATUS_IDLE);
        assert_eq!(status_color("dnd"), STATUS_DND);
        assert_eq!(status_color("offline"), STATUS_OFFLINE);
        assert_eq!(status_color("unknown"), STATUS_OFFLINE);
    }

    #[test]
    fn default_role_color_hex_wraps_around() {
        // The palette has 18 entries; index 18 should wrap to index 0.
        let c0 = default_role_color_hex(0);
        let c18 = default_role_color_hex(18);
        assert_eq!(c0, c18, "palette should wrap modulo 18");
    }
}
