//! Home tab: pinned folders and recent files.

use egui::{Align2, CornerRadius, Rect, Sense, vec2};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, panels::human_size, widgets};

pub mod folders_ui;

/// A soft shadow that rises on hover and settles when the card is pressed.
pub(crate) fn card_lift(ui: &egui::Ui, t: &Tokens, rect: Rect, press: theme::Press) {
    let lift = press.hover * (1.0 - press.down);
    if lift > 0.04 {
        let shadow = egui::Shadow { offset: [0, 3], blur: (14.0 * lift) as u8, spread: 0, color: t.page_shadow.gamma_multiply(0.85 * lift) };
        ui.painter().add(shadow.as_shape(rect, CornerRadius::same(10)));
    }
}

/// Draw the Home page with pinned folders and recent documents.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 36, right: 36, top: 28, bottom: 28 }).show(ui, |ui| {
            #[cfg(not(target_arch = "wasm32"))]
            {
                crate::folders_ui::section(app, ui);
            }

            let mut clear = false;
            // Preferences can turn the list off (#5572).
            if app.recent_limit > 0 {
                ui.add_space(26.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!("Recent")).font(theme::semibold(17.0)));
                    if !app.recent.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let resp = widgets::ghost_button(ui, "trash-2", tl!("Clear"));
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Clear Recent Files")));
                            clear = resp.on_hover_text(tl!("Clear Recent Files")).clicked();
                        });
                    }
                });
                ui.add_space(8.0);
                if app.recent.is_empty() {
                    ui.label(egui::RichText::new(tl!("Files you open in PdfCraft appear here. Drop a PDF anywhere to open it.")).color(t.text_muted));
                }
            }
            let mut open = None;
            let mut remove = None;
            #[cfg(not(target_arch = "wasm32"))]
            let mut reveal = None;
            for r in app.recent.iter().take(app.recent_limit) {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &r.name));
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // The Menu key or Shift+F10 on the focused row opens the same menu a secondary
                    // click does, under the row, so Reveal works from the keyboard.
                    let by_key =
                        resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Menu) || (i.modifiers.shift && i.key_pressed(egui::Key::F10)));
                    let mut popup = egui::Popup::context_menu(&resp);
                    if by_key {
                        popup = popup.open_memory(egui::SetOpenCommand::Bool(true)).anchor(rect).align(egui::RectAlign::BOTTOM_START);
                    }
                    popup.show(|ui| {
                        let label = if cfg!(target_os = "macos") { tl!("Reveal in Finder") } else { tl!("Reveal in File Explorer") };
                        if ui.button(label).clicked() {
                            reveal = Some(r.path.clone());
                            ui.close();
                        }
                    });
                }
                let press = theme::Press::track(ui, &resp);
                // The row, not its response: the pointer stays "in" the row over its remove button.
                let hovered = ui.rect_contains_pointer(rect);
                if hovered && press.hover < 0.04 && press.down < 0.04 {
                    // The remove button sits in the row, so the row response may not be hovered.
                    ui.painter().rect_filled(rect, CornerRadius::same(8), t.hover);
                } else {
                    press.wash(ui, rect, 8, false);
                }
                if hovered {
                    // Remove just this file from the list (#430); the file itself is untouched.
                    let x_rect = Rect::from_center_size(rect.right_center() - vec2(26.0, 0.0), vec2(28.0, 28.0));
                    let x = ui.interact(x_rect, ui.scope_id().with(("remove-recent", &r.path)), Sense::click());
                    let label = crate::i18n::fmt(tl!("Remove {name} from Recent"), &[("name", &r.name)]);
                    x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
                    if x.hovered() {
                        ui.painter().rect_filled(x_rect, CornerRadius::same(6), t.pressed);
                    }
                    icons::paint(ui, x_rect.shrink(6.0), "x", 16.0, t.icon);
                    if x.on_hover_text(tl!("Remove from Recent")).clicked() {
                        remove = Some(r.path.clone());
                    }
                }
                let body = rect.translate(press.offset());
                icons::paint(
                    ui,
                    Rect::from_min_size(body.min + vec2(10.0, 11.0), vec2(24.0, 24.0)),
                    "file-text",
                    22.0,
                    egui::Color32::from_rgb(0xE0, 0x3E, 0x3E),
                );
                // Room on the right for the remove button.
                let detail = ui.painter().text(
                    body.right_center() - vec2(48.0, 0.0),
                    Align2::RIGHT_CENTER,
                    format!("{}  ·  {}", crate::i18n::trn(crate::i18n::current(), r.pages as u64, "{n} page", "{n} pages"), human_size(r.size)),
                    theme::regular(12.0),
                    t.text_muted,
                );
                // The name and path stop short of the page count; hovering shows the whole path.
                let width = detail.left() - 16.0 - (body.left() + 46.0);
                widgets::row_text(ui, body.min + vec2(46.0, 15.0), r.name.as_str(), theme::medium(13.5), t.text, width);
                widgets::row_text(ui, body.min + vec2(46.0, 32.0), r.path.as_str(), theme::regular(11.0), t.text_faint, width);
                if theme::hand(resp).on_hover_text(&r.path).clicked() {
                    open = Some(r.path.clone());
                }
            }
            if let Some(p) = open {
                app.open_recent(&p);
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(p) = reveal {
                app.reveal_path(std::path::Path::new(&p));
            }
            if let Some(p) = remove {
                app.recent.retain(|r| r.path != p);
            }
            if clear {
                app.execute("file.clear_recent");
            }
            ui.add_space(20.0);
            widgets::section_title(ui, tl!("Privacy"));
            ui.label(
                egui::RichText::new(tl!("PdfCraft works offline. No telemetry, no account, and no cloud processing unless you add a provider."))
                    .color(t.text_muted),
            );
        });
    });
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn home_harness() -> egui_kittest::Harness<'static, PdfCraftApp> {
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let mut fonts = false;
        egui_kittest::Harness::builder().with_size(vec2(1500.0, 1200.0)).build_ui_state(
            move |ui, app| {
                if !fonts {
                    ui.ctx().set_fonts(theme::font_definitions());
                    fonts = true;
                }
                show(app, ui);
            },
            PdfCraftApp::new(),
        )
    }

    #[test]
    fn a_focused_recent_row_opens_its_reveal_menu_from_the_keyboard() {
        use egui_kittest::kittest::Queryable;
        for chord in [(egui::Modifiers::NONE, egui::Key::Menu), (egui::Modifiers::SHIFT, egui::Key::F10)] {
            let mut h = home_harness();
            h.state_mut().recent = vec![crate::RecentFile { name: "notes.pdf".into(), path: "/nowhere/notes.pdf".into(), pages: 2, size: 2048 }];
            h.run_steps(3);
            assert!(!egui::Popup::is_any_open(&h.ctx));
            h.get_by_label("notes.pdf").focus();
            h.run_steps(2);
            h.key_press_modifiers(chord.0, chord.1);
            h.run_steps(3);
            assert!(egui::Popup::is_any_open(&h.ctx), "{chord:?} did not open the Reveal menu");
        }
    }
}
