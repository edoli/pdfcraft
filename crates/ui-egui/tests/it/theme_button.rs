//! The title bar keeps Preferences as its only utility button.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(6);
    h
}

fn preference(h: &Harness<'static, PdfCraftApp>) -> String {
    format!("{:?}", h.state().theme_preference)
}

#[test]
fn preferences_gear_opens_preferences() {
    let mut h = harness();
    for label in ["Discord", "Keyboard shortcuts", "Display theme: Use system setting"] {
        assert!(h.query_by_label(label).is_none(), "{label} should be absent");
    }
    h.get_by_label("Preferences").click();
    h.run_steps(6);
    assert!(h.query_by_label("Appearance mode").is_some());
    assert!(h.query_by_label("Light theme").is_some());
    assert!(h.query_by_label("Dark theme").is_some());
    assert_eq!(preference(&h), "System");
}
