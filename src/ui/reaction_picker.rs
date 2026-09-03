//! Emoji picker for reactions: a Discord-style popup grid with a search
//! box, rendered on demand next to a message's hover actions.
//!
//! The set is a curated list of commonly used reaction emoji (smileys,
//! gestures, hearts, symbols). Each renders as color Twemoji; the search
//! filters by the short name shown in the tooltip.

use egui::{Key, Rect, Sense, Ui, Vec2};

use crate::colors;

/// (emoji, short name). Order is the display order in the grid.
pub const PICKER_EMOJIS: &[(&str, &str)] = &[
    // Smileys
    ("😄", "smile"), ("😁", "grin"), ("😂", "joy"), ("🤣", "rofl"),
    ("😊", "blush"), ("🙂", "slight smile"), ("😉", "wink"), ("😍", "heart eyes"),
    ("😘", "kissing heart"), ("😜", "tongue wink"), ("🤪", "zany"), ("🤔", "thinking"),
    ("🤨", "raised eyebrow"), ("😐", "neutral"), ("😶", "no mouth"), ("🙄", "rolling eyes"),
    ("😴", "sleeping"), ("🤤", "drool"), ("😵", "dizzy"), ("🥳", "party"),
    ("😎", "cool"), ("🤓", "nerd"), ("🥸", "disguise"), ("😕", "confused"),
    ("😟", "worried"), ("🙁", "frown"), ("😢", "cry"), ("😭", "sob"),
    ("😤", "triumph"), ("😡", "rage"), ("🤬", "cursing"), ("😱", "scream"),
    ("😳", "flushed"), ("🥵", "hot"), ("🥶", "cold"), ("😷", "mask"),
    ("🤒", "thermometer"), ("🤕", "bandage"), ("🤢", "sick"), ("🤮", "vomit"),
    // Gestures / people
    ("👍", "thumbs up"), ("👎", "thumbs down"), ("👌", "ok"), ("🤌", "pinch"),
    ("✌️", "peace"), ("🤞", "fingers crossed"), ("🤟", "love you"), ("🤘", "rock on"),
    ("👏", "clap"), ("🙌", "raise hands"), ("🙏", "pray"), ("💪", "muscle"),
    ("🤝", "handshake"), ("👋", "wave"), ("🫶", "heart hands"), ("🤙", "call me"),
    ("☝️", "point up"), ("👉", "point right"), ("👑", "crown"), ("🐱", "cat"),
    ("🐶", "dog"), ("🦊", "fox"), ("🐻", "bear"), ("🐼", "panda"),
    // Hearts / symbols
    ("❤️", "red heart"), ("🧡", "orange heart"), ("💛", "yellow heart"),
    ("💚", "green heart"), ("💙", "blue heart"), ("💜", "purple heart"),
    ("🖤", "black heart"), ("🤍", "white heart"), ("💯", "hundred"),
    ("🔥", "fire"), ("⭐", "star"), ("✨", "sparkles"), ("💫", "dizzy star"),
    ("💥", "boom"), ("⚡", "zap"), ("🌈", "rainbow"), ("☀️", "sun"),
    ("🌙", "moon"), ("💎", "gem"), ("🏆", "trophy"), ("🥇", "gold medal"),
    ("🎉", "tada"), ("🎊", "confetti"), ("🎈", "balloon"), ("🎁", "gift"),
    // Objects / misc
    ("👀", "eyes"), ("🧠", "brain"), ("🗣️", "speaking head"), ("💬", "speech bubble"),
    ("💭", "thought"), ("💤", "zzz"), ("🔔", "bell"), ("📌", "pin"),
    ("📎", "paperclip"), ("🔒", "lock"), ("🔑", "key"), ("🔨", "hammer"),
    ("🛠️", "tools"), ("⚙️", "gear"), ("🧪", "test tube"), ("💻", "laptop"),
    ("📱", "phone"), ("🎮", "game"), ("🎧", "headphones"), ("🎵", "note"),
    ("🎬", "clapper"), ("📷", "camera"), ("🍕", "pizza"), ("🍔", "burger"),
    ("🍟", "fries"), ("🍿", "popcorn"), ("🍩", "donut"), ("🍪", "cookie"),
    ("🎂", "cake"), ("☕", "coffee"), ("🍵", "tea"), ("🍺", "beer"),
    ("🍷", "wine"), ("🥤", "cup"), ("✅", "check"), ("❌", "cross"),
    ("❓", "question"), ("❗", "exclamation"), ("⚠️", "warning"), ("🚫", "prohibited"),
    ("🆗", "ok button"), ("🔴", "red circle"), ("🟠", "orange circle"),
    ("🟡", "yellow circle"), ("🟢", "green circle"), ("🔵", "blue circle"),
    ("🟣", "purple circle"), ("⚫", "black circle"), ("⚪", "white circle"),
];

/// Draw the picker popup anchored at `anchor` for `message_id`.
/// `for_message` / `search` are ChatState fields (owned by the caller);
/// the picker clears them when it closes. Returns the chosen emoji when
/// the user clicks one; the caller then sends the reaction.
pub fn show(
    ui: &mut Ui,
    for_message: &mut Option<crate::model::Snowflake>,
    search: &mut String,
    opened_at: Option<std::time::Instant>,
    message_id: crate::model::Snowflake,
    anchor: egui::Pos2,
) -> Option<String> {
    if *for_message != Some(message_id) {
        return None; // picker belongs to a different message
    }
    // Grace window: the click that opened the picker is still "clicked"
    // in this very frame; outside-click handling must ignore it or the
    // picker flash-closes (same class of bug the settings modal had).
    let in_grace = opened_at
        .map(|t| t.elapsed() < std::time::Duration::from_millis(250))
        .unwrap_or(true);

    let cell = 34.0;
    let cols = 9.0f32;
    let grid_w = cols * cell + 16.0;
    let shown: Vec<&(&str, &str)> = PICKER_EMOJIS
        .iter()
        .filter(|(_, name)| search.is_empty() || name.contains(search.to_lowercase().trim()))
        .take(72)
        .collect();
    let rows = (shown.len() as f32 / cols).ceil().max(1.0);
    let grid_h = rows * cell + 12.0;
    let total_h = 44.0 + grid_h;

    // Keep the popup inside the viewport.
    let vp = ui.ctx().viewport_rect();
    let mut pos = anchor + egui::vec2(-grid_w - 8.0, -total_h - 8.0);
    pos.x = pos.x.clamp(vp.min.x + 4.0, vp.max.x - grid_w - 4.0);
    pos.y = pos.y.clamp(vp.min.y + 4.0, vp.max.y - total_h - 4.0);
    let area_rect = Rect::from_min_size(pos, egui::vec2(grid_w, total_h));

    let mut chosen: Option<String> = None;
    let frame = egui::Frame::new()
        .fill(colors::BG_FLOATING)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(8))
        .stroke(egui::Stroke::new(1.0, colors::BG_INPUT));
    egui::Area::new(egui::Id::new("reaction_picker"))
        .order(egui::Order::Foreground)
        .fixed_pos(area_rect.min)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(grid_w - 16.0);
                ui.vertical(|ui| {
                    // Search row.
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        crate::icons::draw(ui.painter(), "search", r.center(), 15.0, colors::TEXT_TERTIARY);
                        let resp = ui.add(
                            egui::TextEdit::singleline(search)
                                .desired_width(grid_w - 52.0)
                                .hint_text("Search emoji")
                                .font(egui::FontId::proportional(12.5)),
                        );
                        resp.request_focus();
                    });
                    ui.add_space(6.0);
                    // Grid.
                    let grid_rect = Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(grid_w - 16.0, grid_h),
                    );
                    let mut cursor = grid_rect.min;
                    for (i, (emo, name)) in shown.iter().enumerate() {
                        let cell_rect = Rect::from_min_size(cursor, Vec2::splat(cell));
                        let resp = ui
                            .interact(cell_rect, ui.id().with("emo").with(i), Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(*name);
                        if resp.hovered() {
                            ui.painter().rect_filled(
                                cell_rect.shrink(2.0),
                                6.0,
                                colors::BG_INPUT,
                            );
                        }
                        let img_rect = Rect::from_center_size(
                            cell_rect.center(),
                            Vec2::splat(24.0),
                        );
                        crate::ui::emoji::draw_emoji_at(ui, img_rect, emo);
                        if resp.clicked() {
                            chosen = Some((*emo).to_string());
                        }
                        cursor.x += cell;
                        if (i + 1) as f32 % cols == 0.0 {
                            cursor.x = grid_rect.min.x;
                            cursor.y += cell;
                        }
                    }
                    // Consume the reserved grid area so the popup layout is
                    // honest about its size.
                    ui.allocate_rect(grid_rect, Sense::hover());
                });
            });
        });

    // Close keys. Outside-click only after the grace window: the click
    // that opened the picker is still "clicked" in this very frame and
    // would otherwise close it instantly (the settings-modal bug class).
    let close = ui.input(|i| i.key_pressed(Key::Escape))
        || (!in_grace
            && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary) && {
                let p = i.pointer.interact_pos();
                p.map(|p| !area_rect.contains(p)).unwrap_or(false)
            }));
    if close && chosen.is_none() {
        *for_message = None;
        search.clear();
    }
    chosen
}
