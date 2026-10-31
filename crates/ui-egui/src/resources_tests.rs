//! Shift+I opens Assets even though the obsolete Resources button is absent from the toolbar.

use egui::accesskit::Role;
use egui::{Event, Key, Modifiers};
use lizard_engine::Session;

use crate::LeftTab;
use crate::layers_drag_tests::Ui;

fn shift_i(u: &mut Ui) {
    let modifiers = Modifiers::SHIFT;
    u.frame(vec![
        Event::Key { key: Key::I, physical_key: None, pressed: true, repeat: false, modifiers },
        Event::Key { key: Key::I, physical_key: None, pressed: false, repeat: false, modifiers },
    ]);
    u.frame(vec![]);
}

fn assets_shown(u: &Ui) -> bool {
    u.app.ui.left_tab == LeftTab::Assets && !u.app.ui.left_collapsed && !u.app.ui.versions.open
}

/// The three states the sidebar can be in when Assets is requested.
fn starts() -> Vec<(&'static str, Ui)> {
    let shown = Ui::new(Session::new());
    let mut collapsed = Ui::new(Session::new());
    collapsed.app.ui.left_collapsed = true;
    let mut versions = Ui::new(Session::new());
    crate::versions::open(&mut versions.app);
    vec![("layers", shown), ("collapsed", collapsed), ("version history", versions)]
}

#[test]
fn the_toolbar_has_no_resources_button() {
    let mut u = Ui::new(Session::new());
    u.ctx.enable_accesskit();
    u.frame(vec![]);
    let out = u.frame_output(vec![]);
    let nodes = out.platform_output.accesskit_update.expect("an AccessKit update").nodes;
    assert!(!nodes.iter().any(|(_, n)| n.role() == Role::Button && n.label() == Some("Resources")));
}

#[test]
fn shift_i_opens_the_assets_panel() {
    for (name, mut u) in starts() {
        u.frame(vec![]);
        shift_i(&mut u);
        assert!(assets_shown(&u), "{name}: the Assets panel is open");
    }
}
