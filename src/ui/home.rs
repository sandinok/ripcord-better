//! Home center pages: Friends, Message Requests, Nitro, Shop, Quests.
//!
//! The nav lives in the sidebar's DM list. Each page renders REAL data
//! where the bot token allows it (relationships, own Nitro status) and
//! honest "not available for bot accounts" states where Discord's user
//! APIs refuse bots (store, quests).

use egui::{Sense, Ui, Vec2};

use crate::colors;
use crate::state::AppState;

/// ctx-temp marker: which home page is selected.
pub const HOME_PAGE: &str = "home_page";

pub fn active_page(ui: &Ui) -> Option<String> {
    ui.ctx()
        .data(|d| d.get_temp::<String>(egui::Id::new(HOME_PAGE)))
}

pub fn clear(ui: &Ui) {
    ui.ctx()
        .data_mut(|d| d.remove_temp::<String>(egui::Id::new(HOME_PAGE)));
}

/// Render the active home page into the chat area (called when no channel
/// is selected and a page marker is set).
pub fn render(ui: &mut Ui, app_state: &AppState, page: &str) {
    ui.set_min_width(ui.available_width());
    ui.set_min_height(ui.available_height());
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let _ = rect;
    ui.vertical_centered(|ui| {
        ui.set_width(ui.available_width());
        ui.add_space(48.0);
        match page {
            "friends" => friends_page(ui, app_state),
            "nitro" => nitro_page(ui, app_state),
            "shop" | "quests" | "requests" => simple_page(ui, page),
            _ => {}
        }
    });
}

fn page_header(ui: &mut Ui, icon: &str, title: &str, subtitle: &str) {
    crate::icons::draw(
        ui.painter(),
        icon,
        egui::pos2(ui.next_widget_position().x + 16.0, ui.next_widget_position().y + 20.0),
        30.0,
        colors::BLURPLE,
    );
    ui.add_space(46.0);
    ui.label(
        egui::RichText::new(title)
            .size(22.0)
            .strong()
            .color(colors::TEXT_HEADER),
    );
    ui.label(
        egui::RichText::new(subtitle)
            .size(13.5)
            .color(colors::TEXT_TERTIARY),
    );
    ui.add_space(24.0);
}

/// Friends: GET /users/@me/relationships. Bot accounts get a real 403
/// from Discord for this endpoint, which we show as-is.
fn friends_page(ui: &mut Ui, app_state: &AppState) {
    page_header(
        ui,
        "person",
        "Friends",
        "Your friend list, straight from the Discord API.",
    );
    let key = egui::Id::new("relationships_fetched");
    let fetched = ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    if !fetched {
        ui.ctx().data_mut(|d| d.insert_temp(key, true));
        if let Some(rest) = crate::rest::global() {
            tokio::spawn(async move {
                match rest.get_relationships().await {
                    Ok(rels) => {
                        // Touch every user record so the list can render.
                        if let Some(s) = crate::state::global() {
                            for r in &rels {
                                if let Some(u) = r.get("user").cloned() {
                                    if let Ok(user) =
                                        serde_json::from_value::<crate::model::User>(u)
                                    {
                                        s.touch_user(&user);
                                    }
                                }
                            }
                            s.set_relationships_len(rels.len());
                        }
                    }
                    Err(e) => {
                        tracing::info!(error = %e, "relationships unavailable (bot token)");
                        if let Some(s) = crate::state::global() {
                            s.set_relationships_unavailable(format!("{e}"));
                        }
                    }
                }
            });
        }
    }
    if let Some(err) = app_state.relationships_unavailable() {
        ui.label(
            egui::RichText::new(format!(
                "Discord does not serve the friends list to bot tokens:\n{}",
                shorten(&err, 90)
            ))
            .size(13.0)
            .color(colors::TEXT_TERTIARY),
        );
        return;
    }
    let count = app_state.relationships_len();
    if count == 0 {
        spinner(ui);
        return;
    }
    ui.label(
        egui::RichText::new(format!("{count} friends"))
            .size(13.0)
            .color(colors::TEXT_SECONDARY),
    );
}

fn nitro_page(ui: &mut Ui, app_state: &AppState) {
    page_header(
        ui,
        "diamond",
        "Nitro",
        "Your subscription status and perks.",
    );
    let me = app_state.current_user();
    let premium = me.as_ref().and_then(|u| u.premium_type).unwrap_or(0);
    let label = match premium {
        1 => "Nitro Classic",
        2 => "Nitro",
        3 => "Nitro Basic",
        _ => "No subscription",
    };
    // Gem row for flair.
    ui.horizontal(|ui| {
        for _ in 0..(if premium > 0 { 3 } else { 0 }) {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            crate::icons::draw(ui.painter(), "diamond", r.center(), 22.0, colors::BLURPLE);
        }
    });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(label)
            .size(17.0)
            .strong()
            .color(colors::TEXT_PRIMARY),
    );
    if premium == 0 {
        ui.label(
            egui::RichText::new(
                "This account has no Nitro. Upgrades happen through Discord's\ncheckout, which requires a user account.",
            )
            .size(13.0)
            .color(colors::TEXT_TERTIARY),
        );
    }
}

fn simple_page(ui: &mut Ui, page: &str) {
    let (icon, title, body) = match page {
        "shop" => (
            "storefront",
            "Shop",
            "The Discord shop is a user-account storefront (billing, currency,\nregional pricing). Bot tokens cannot browse it, so Basalt shows\nthis page instead of faking a catalog.",
        ),
        "quests" => (
            "sports_esports",
            "Quests",
            "Quests are tied to user accounts and their game activity.\nThere is nothing a bot token can claim here.",
        ),
        _ => (
            "person_add",
            "Message Requests",
            "Message-request filtering lives in user-account settings;\nDMs this account already has appear in the sidebar.",
        ),
    };
    page_header(ui, icon, title, "");
    ui.label(
        egui::RichText::new(body)
            .size(13.5)
            .color(colors::TEXT_SECONDARY),
    );
}

fn spinner(ui: &mut Ui) {
    ui.add_space(12.0);
    let t = ui.input(|i| i.time) as f32;
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let start = t * 3.0;
    painter.circle_stroke(
        rect.center(),
        10.0,
        egui::Stroke::new(2.5, colors::TEXT_TERTIARY),
    );
    painter.line_segment(
        [
            rect.center(),
            rect.center() + egui::vec2(start.cos() * 9.0, start.sin() * 9.0),
        ],
        egui::Stroke::new(2.5, colors::BLURPLE),
    );
}

fn shorten(s: &str, n: usize) -> String {
    let out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{out}...")
    } else {
        out
    }
}
