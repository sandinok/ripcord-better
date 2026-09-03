//! User settings modal - Discord-style overlay. Left nav + content.
//! Sections: My Account, Appearance, Notifications, About. Every control
//! is wired: the appearance toggles restyle the chat immediately, the
//! notification toggles drive the sidebar badges and the window title,
//! sign out clears the session.

use egui::{Sense, Ui, Vec2};

use crate::colors;
use crate::config::Config;
use crate::gateway::Outbound;
use crate::state::AppState;
use crate::ui::emoji;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Account,
    Appearance,
    Notifications,
    About,
}

pub struct SettingsState {
    pub open: bool,
    pub section: Section,
    /// 0..=1 entrance animation progress.
    pub enter: f32,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            open: false,
            section: Section::Account,
            enter: 0.0,
        }
    }
}

impl SettingsState {
    pub fn open(&mut self) {
        self.open = true;
        self.enter = 0.0;
    }
}

/// Returns true when the user asked to sign out.
pub fn render(
    ui: &mut Ui,
    settings: &mut SettingsState,
    config: &mut Config,
    shared: &AppState,
    gateway_tx: &tokio::sync::mpsc::UnboundedSender<Outbound>,
) -> bool {
    let mut sign_out = false;

    let dt = crate::ui::scroll::dt_from_ctx(ui.ctx()).min(0.040);
    settings.enter = (settings.enter + dt / 0.22).min(1.0);
    let t = crate::ui::scroll::ease_out_cubic(settings.enter);

    // The hosting Area has an unconstrained max_rect; anchor everything to
    // the real viewport.
    let full = ui.ctx().viewport_rect();
    let painter = ui.painter().clone();

    // Backdrop.
    let backdrop_alpha = (200.0 * t) as u8;
    painter.rect_filled(full, 0.0, egui::Color32::from_black_alpha(backdrop_alpha));

    // Content card.
    let card_w = (full.width() * 0.72).clamp(560.0, 920.0);
    let card_h = (full.height() * 0.78).clamp(400.0, 620.0);
    let slide = (1.0 - t) * 24.0;
    let card_rect = egui::Rect::from_center_size(
        full.center() - egui::vec2(slide, 0.0),
        egui::vec2(card_w, card_h),
    );
    // Shadow + card.
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 6.0)),
        12.0,
        egui::Color32::from_black_alpha((70.0 * t) as u8),
    );
    painter.rect_filled(card_rect, 12.0, colors::BG_FLOATING);

    // Backdrop click (outside the card) closes. NOTE: we deliberately do
    // NOT register a full-screen interact widget - it would swallow every
    // click inside the card. Instead we read the raw pointer state.
    let backdrop_clicked = ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary))
        && ui
            .input(|i| i.pointer.interact_pos())
            .map(|pos| !card_rect.contains(pos))
            .unwrap_or(false);
    if backdrop_clicked {
        settings.open = false;
        shared.set_settings_open(false);
    }
    // ESC closes.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        settings.open = false;
        shared.set_settings_open(false);
    }

    // Explicit two-column layout: nav (left 220px) + content (rest).
    let nav_w = 220.0;
    let nav_rect = egui::Rect::from_min_max(
        card_rect.min,
        egui::pos2(card_rect.min.x + nav_w, card_rect.max.y),
    );
    let content_rect = egui::Rect::from_min_max(
        egui::pos2(card_rect.min.x + nav_w, card_rect.min.y),
        card_rect.max,
    );
    // Column divider.
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(nav_rect.max.x - 1.0, nav_rect.min.y),
            egui::pos2(nav_rect.max.x, nav_rect.max.y),
        ),
        0.0,
        colors::BG_ACCENT,
    );

    crate::ui::allocate_ui_at_rect(ui, nav_rect, |ui| {
        ui.set_min_width(ui.available_width());
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(
                egui::RichText::new("User Settings")
                    .size(15.0)
                    .strong()
                    .color(colors::TEXT_HEADER),
            );
        });
        ui.add_space(12.0);
        nav_item(ui, settings, Section::Account, "account_circle", "My Account");
        nav_item(ui, settings, Section::Appearance, "palette", "Appearance");
        nav_item(ui, settings, Section::Notifications, "notifications", "Notifications");
        nav_item(ui, settings, Section::About, "help", "About");

        // Pin the close button to the bottom of the card.
        let x_rect = egui::Rect::from_min_size(
            egui::pos2(nav_rect.min.x + 16.0, nav_rect.max.y - 52.0),
            egui::vec2(nav_w - 32.0, 36.0),
        );
        let resp = ui
            .interact(x_rect, ui.id().with("settings_close"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter_at(x_rect);
        p.rect_filled(x_rect, 6.0, colors::RED.gamma_multiply(0.85));
        crate::icons::draw(&p, "close", x_rect.center(), 18.0, colors::TEXT_PRIMARY);
        if resp.clicked() {
            settings.open = false;
            shared.set_settings_open(false);
        }
    });

    crate::ui::allocate_ui_at_rect(ui, content_rect, |ui| {
        ui.set_min_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.add_space(20.0);
                match settings.section {
                    Section::Account => account_section(ui, shared, gateway_tx, &mut sign_out),
                    Section::Appearance => appearance_section(ui, config),
                    Section::Notifications => notifications_section(ui, config),
                    Section::About => about_section(ui),
                }
                ui.add_space(20.0);
            });
    });

    sign_out
}

fn nav_item(ui: &mut Ui, settings: &mut SettingsState, section: Section, icon: &str, label: &str) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label);
    let selected = settings.section == section;
    let painter = ui.painter_at(rect);
    if resp.hovered() || selected {
        painter.rect_filled(rect, 6.0, colors::BG_ACCENT.gamma_multiply(0.45));
    }
    if selected {
        // Left accent.
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(rect.min.x, rect.center().y - 10.0), egui::vec2(3.0, 20.0)),
            2.0,
            colors::TEXT_PRIMARY,
        );
    }
    let color = if selected { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY };
    crate::icons::draw(&painter, icon, egui::pos2(rect.min.x + 26.0, rect.center().y), 20.0, color);
    painter.text(
        egui::pos2(rect.min.x + 48.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        color,
    );
    if resp.clicked() {
        settings.section = section;
    }
}

// ───────────────────────────── sections ─────────────────────────────

fn account_section(
    ui: &mut Ui,
    shared: &AppState,
    gateway_tx: &tokio::sync::mpsc::UnboundedSender<Outbound>,
    sign_out: &mut bool,
) {
    section_title(ui, "My Account");
    let user = shared.current_user();

    let frame = egui::Frame::new()
        .fill(colors::BG_SECONDARY_ALT)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(16));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            if let Some(u) = &user {
                crate::image_loader::render_avatar(ui, &u.avatar_url(), 72.0, u.display_name(), Some(&shared.own_status()));
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                if let Some(u) = &user {
                    ui.label(
                        egui::RichText::new(u.display_name())
                            .size(19.0)
                            .strong()
                            .color(colors::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new(format!("@{}", u.username))
                            .size(13.0)
                            .color(colors::TEXT_TERTIARY),
                    );
                    if u.bot {
                        ui.label(
                            egui::RichText::new("Bot account")
                                .size(12.0)
                                .color(colors::TEXT_MUTED),
                        );
                    }
                } else {
                    ui.label(egui::RichText::new("Not signed in").size(15.0).color(colors::TEXT_TERTIARY));
                }
            });
        });
    });

    ui.add_space(16.0);
    sub_title(ui, "Status");
    let mut new_status: Option<String> = None;
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        for (key, label) in [
            ("online", "Online"),
            ("idle", "Idle"),
            ("dnd", "Do not disturb"),
            ("invisible", "Invisible"),
        ] {
            let selected = shared.own_status() == key;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(150.0, 34.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let painter = ui.painter_at(rect);
            let bg = if selected {
                colors::BLURPLE
            } else if resp.hovered() {
                colors::BG_ACCENT
            } else {
                colors::BG_SECONDARY_ALT
            };
            painter.rect_filled(rect, 6.0, bg);
            let color = crate::colors::status_color(key);
            painter.circle_filled(egui::pos2(rect.min.x + 16.0, rect.center().y), 6.0, color);
            if key == "invisible" {
                painter.circle_filled(egui::pos2(rect.min.x + 16.0, rect.center().y), 3.0, colors::BG_FLOATING);
            }
            painter.text(
                egui::pos2(rect.min.x + 30.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(13.0),
                colors::TEXT_PRIMARY,
            );
            if resp.clicked() {
                new_status = Some(key.to_string());
            }
        }
    });
    if let Some(st) = new_status {
        shared.request_presence(&st);
        let _ = gateway_tx.send(Outbound::SetPresence { status: st, afk: false });
        ui.ctx().request_repaint();
    }

    ui.add_space(16.0);
    sub_title(ui, "Session");
    ui.horizontal(|ui| {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(160.0, 38.0), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let painter = ui.painter_at(rect);
        let bg = if resp.hovered() { colors::RED_HOVER } else { colors::RED };
        painter.rect_filled(rect, 6.0, bg);
        crate::icons::draw(&painter, "logout", egui::pos2(rect.min.x + 20.0, rect.center().y), 18.0, colors::TEXT_PRIMARY);
        painter.text(
            egui::pos2(rect.min.x + 38.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Sign out",
            egui::FontId::proportional(14.0),
            colors::TEXT_PRIMARY,
        );
        if resp.clicked() {
            *sign_out = true;
        }
    });
}

fn appearance_section(ui: &mut Ui, config: &mut Config) {
    section_title(ui, "Appearance");

    sub_title(ui, "Message font size");
    ui.horizontal(|ui| {
        let mut size = config.font_size;
        ui.add(egui::Slider::new(&mut size, 12.0..=19.0).text("px"));
        config.font_size = size.round();
    });
    // Live preview.
    ui.add_space(6.0);
    let frame = egui::Frame::new()
        .fill(colors::BG_CHAT)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(12));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("Basalt Test Bot")
                    .size(config.font_size)
                    .strong()
                    .color(colors::TEXT_PRIMARY),
            );
            emoji::render_label(
                ui,
                "This is how your messages will look. Emojis stay in color: 🔥 🚀",
                config.font_size,
                colors::TEXT_SECONDARY,
            );
        });
    });

    ui.add_space(16.0);
    sub_title(ui, "Message density");
    ui.horizontal(|ui| {
        for (key, label) in [("cozy", "Cozy"), ("compact", "Compact")] {
            let selected = config.density == key;
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(120.0, 34.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let painter = ui.painter_at(rect);
            let bg = if selected {
                colors::BLURPLE
            } else if resp.hovered() {
                colors::BG_ACCENT
            } else {
                colors::BG_SECONDARY_ALT
            };
            painter.rect_filled(rect, 6.0, bg);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(13.0),
                colors::TEXT_PRIMARY,
            );
            if resp.clicked() {
                config.density = key.to_string();
                let _ = config.save();
            }
        }
    });

    ui.add_space(16.0);
    sub_title(ui, "Layout");
    let mut show_members = config.show_members;
    toggle_row(ui, "Show member list", &mut show_members);
    if show_members != config.show_members {
        config.show_members = show_members;
        let _ = config.save();
    }
}

fn notifications_section(ui: &mut Ui, config: &mut Config) {
    section_title(ui, "Notifications");

    ui.label(
        egui::RichText::new(
            "Basalt keeps notifications inside the app: unread dots and mention \
             badges on channels and servers, plus a mention counter in the window \
             title. No system tray, no sounds, nothing leaves your machine.",
        )
        .size(13.0)
        .color(colors::TEXT_TERTIARY),
    );
    ui.add_space(10.0);

    let mut badges = config.show_unread_badges;
    toggle_row(ui, "Unread badges on channels and servers", &mut badges);
    if badges != config.show_unread_badges {
        config.show_unread_badges = badges;
        let _ = config.save();
    }
    let mut title = config.title_mentions;
    toggle_row(ui, "Show mention count in the window title", &mut title);
    if title != config.title_mentions {
        config.title_mentions = title;
        let _ = config.save();
    }
}

fn about_section(ui: &mut Ui) {
    section_title(ui, "About Basalt");
    crate::ui::draw_basalt_logo(ui.painter(), 64.0, ui.next_widget_position().y + 40.0, 72.0);
    ui.add_space(88.0);
    ui.label(
        egui::RichText::new(format!("Basalt {}", env!("CARGO_PKG_VERSION")))
            .size(20.0)
            .strong()
            .color(colors::TEXT_PRIMARY),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("A native Rust + egui Discord client. No Electron, no WebView.")
            .size(13.5)
            .color(colors::TEXT_TERTIARY),
    );
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new("Built with Rust, egui, tokio, and Material Symbols icons.")
            .size(12.5)
            .color(colors::TEXT_MUTED),
    );
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Emoji art: Twemoji (jdecked/twemoji, CC-BY 4.0).")
            .size(12.5)
            .color(colors::TEXT_MUTED),
    );
    ui.add_space(12.0);
    // Repo link (opens the browser).
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(280.0, 36.0), Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Open in your browser");
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, colors::BLURPLE);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "github.com/sandinok/basalt",
        egui::FontId::proportional(13.5),
        colors::TEXT_PRIMARY,
    );
    if resp.clicked() {
        let _ = open::that("https://github.com/sandinok/basalt");
    }
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(
            "MIT license. Not affiliated with Discord Inc. Use of user tokens \
             may violate Discord's Terms of Service; this client is an \
             independent experiment.",
        )
        .size(11.5)
        .color(colors::TEXT_MUTED),
    );
}

// ───────────────────────────── helpers ─────────────────────────────

fn section_title(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(19.0)
            .strong()
            .color(colors::TEXT_HEADER),
    );
    ui.add_space(4.0);
}

fn sub_title(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(12.0)
            .strong()
            .color(colors::TEXT_TERTIARY),
    );
    ui.add_space(6.0);
}

fn toggle_row(ui: &mut Ui, label: &str, value: &mut bool) {
    let row_resp = ui
        .horizontal(|ui| {
            ui.add_space(2.0);
            // Switch.
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(42.0, 24.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let painter = ui.painter_at(rect);
            let (bg, knob_x) = if *value {
                (colors::GREEN, rect.right() - 12.0)
            } else {
                (colors::BG_ACCENT, rect.min.x + 12.0)
            };
            painter.rect_filled(rect, 12.0, bg);
            painter.circle_filled(egui::pos2(knob_x, rect.center().y), 9.0, colors::TEXT_PRIMARY);
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(label)
                    .size(13.5)
                    .color(colors::TEXT_SECONDARY),
            );
            resp
        })
        .inner;
    if row_resp.clicked() {
        *value = !*value;
    }
    ui.add_space(4.0);
}
