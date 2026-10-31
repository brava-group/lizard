//! Popup menus are as wide as their widest row (at least as wide as what opened them), not egui's
//! default area width, and stay inside the window: drawn headless.

use egui::{Color32, Event, Order, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use lizard_engine::Session;

use crate::right::{menu_item, menu_popup};
use crate::theme::{medium, regular};
use crate::{LizardApp, NoServices, RightTab};

const SCREEN: egui::Vec2 = vec2(1440.0, 900.0);

fn input(t: &mut f64, events: Vec<Event>) -> RawInput {
    *t += 1.0 / 60.0;
    RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)), time: Some(*t), events, ..Default::default() }
}

fn press(p: Pos2, pressed: bool) -> Vec<Event> {
    vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Default::default() }]
}

/// The rects of the foreground areas (menus) showing.
fn menus(ctx: &egui::Context) -> Vec<Rect> {
    ctx.memory(|m| m.areas().visible_layer_ids().into_iter().filter(|l| l.order == Order::Foreground).filter_map(|l| m.area_rect(l.id)).collect())
}

/// The popup frame's margins, left plus right.
fn frame_margins(ctx: &egui::Context) -> f32 {
    let _ = ctx;
    // The shared Menu has a 0.5 pt stroke and no horizontal inner margin.
    1.0
}

#[test]
fn a_menu_is_as_wide_as_its_widest_row_and_does_not_wrap() {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx, false, 0);
    let mut t = 0.0;
    let items = [("Copy as code", "Cmd+Shift+C"), ("Paste", "Cmd+V"), ("A much longer menu item label", "")];
    let mut frame = |events: Vec<Event>| {
        let mut out = ctx.run_ui(input(&mut t, events), |ui| {
            // A narrow icon button opens it.
            let (_, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::click());
            menu_popup(&resp).show(|ui| {
                for (label, keys) in items {
                    menu_item(ui, label, keys);
                }
            });
        });
        out.textures_delta.clear();
    };
    let at = pos2(12.0, 12.0);
    frame(vec![]);
    frame(press(at, true));
    frame(press(at, false));
    for _ in 0..3 {
        frame(vec![]);
    }
    let shown = menus(&ctx);
    let [menu] = shown.as_slice() else { panic!("one menu open: {shown:?}") };
    let label_width = |s: &str| ctx.fonts_mut(|f| f.layout_no_wrap(s.into(), medium(11.0), Color32::WHITE).size().x);
    let shortcut_width = |s: &str| ctx.fonts_mut(|f| f.layout_no_wrap(crate::toolbar::pretty_shortcut(s), regular(11.0), Color32::WHITE).size().x);
    let content = items
        .iter()
        .map(|(label, shortcut)| 32.0 + label_width(label) + if shortcut.is_empty() { 0.0 } else { 24.0 + shortcut_width(shortcut) })
        .fold(96.0, f32::max)
        .ceil();
    let expected = content + frame_margins(&ctx);
    assert!((menu.width() - expected).abs() <= 2.0, "menu {} wide, content needs {expected}", menu.width());
    // One line per row.
    let rows = 26.0 * items.len() as f32;
    assert!(menu.height() < rows + 40.0, "menu {} tall for {rows} of rows", menu.height());
}

#[test]
fn right_panel_dropdown_menus_fit_the_panel() {
    let mut app = LizardApp::new(Session::new(), Box::new(NoServices));
    app.ui.right_tab = RightTab::Inspect;
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut frame = |app: &mut LizardApp, events: Vec<Event>| {
        let mut out = ctx.run_ui(input(&mut t, events), |ui| app.ui(ui));
        out.textures_delta.clear();
    };
    for _ in 0..3 {
        frame(&mut app, vec![]);
    }
    // With nothing selected, Inspect lists the frames ready for dev under a filter dropdown, the
    // only control on that panel: click down the panel until it opens.
    let panel_left = SCREEN.x - app.ui.right_width;
    let x = panel_left + app.ui.right_width / 2.0;
    let mut menu = None;
    for y in (140..600).step_by(6) {
        let at = pos2(x, y as f32);
        frame(&mut app, press(at, true));
        frame(&mut app, press(at, false));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        if let [m] = menus(&ctx).as_slice() {
            menu = Some(*m);
            break;
        }
    }
    let menu = menu.expect("the Ready for dev filter menu opens");
    // The dropdown spans the panel less its padding; its three short options fit in that.
    let dropdown = app.ui.right_width - 2.0 * crate::widgets::PAD;
    assert!(menu.width() >= dropdown - 16.0, "menu {menu:?} at least as wide as its dropdown ({dropdown})");
    assert!(menu.width() <= app.ui.right_width, "menu {menu:?} fits the {} px panel", app.ui.right_width);
    assert!(menu.max.x <= SCREEN.x && menu.min.x >= panel_left - 8.0, "menu {menu:?} inside the window, under the panel");
}
