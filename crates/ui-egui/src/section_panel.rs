//! Section-specific Design controls and the status picker shared with Dev Mode.

use crate::primitives::TooltipResponseExt as _;
use egui::{CornerRadius, Sense, Stroke, Ui, vec2};
use lizard_doc::{DevStatus, Document, NodeId, NodeKind};
use serde_json::json;

use crate::LizardApp;
use crate::i18n::{tr, tr_noop};
use crate::icons::Icon;
use crate::right::{common, menu_item, menu_popup, row};
use crate::widgets;

/// Section fill presets: light tints that keep dark text readable.
const SECTION_COLORS: [(&str, &str); 8] = [
    (tr_noop("White"), "#FFFFFF"),
    (tr_noop("Gray"), "#EDEDED"),
    (tr_noop("Red"), "#FFE0DE"),
    (tr_noop("Orange"), "#FFE8D1"),
    (tr_noop("Yellow"), "#FFF4C2"),
    (tr_noop("Green"), "#DDF5E3"),
    (tr_noop("Blue"), "#DCEBFF"),
    (tr_noop("Violet"), "#EBE3FF"),
];

fn is_section(doc: &Document, id: NodeId) -> bool {
    doc.node(id).is_some_and(|n| matches!(n.kind, NodeKind::Section { .. }))
}

pub use lizard_engine::devinspect::has_status;

/// The handoff status of `ids` as a dropdown `width` wide ("Changed" when edited since marked ready,
/// "Mixed" when they differ).
pub fn status_dropdown(app: &mut LizardApp, ui: &mut Ui, doc: &Document, ids: &[NodeId], width: f32) {
    let label = match common(doc, ids, |n| (n.dev_status, n.dev_changed)) {
        Some((DevStatus::ReadyForDev, true)) => tr("Changed"),
        Some((d, _)) => tr(d.label()),
        None => tr("Mixed"),
    };
    let resp = crate::primitives::Select::trigger(ui, label, width, Some(Icon::Code));
    menu_popup(&resp).show(|ui| {
        for d in DevStatus::ALL {
            if menu_item(ui, tr(d.label()), "") {
                app.set_props(ids, json!({"devStatus": d.id()}));
                ui.close();
            }
        }
    });
}

pub fn section(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    if sel.is_empty() || !sel.iter().all(|i| is_section(doc, *i)) {
        return;
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    let ids = sel.to_vec();
    widgets::sublabels(ui, tr("Section color"), None);
    row(ui, 32.0, |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let current = common(doc, sel, |n| n.fills.first().and_then(|p| p.solid_color()).map(|c| c.to_hex()));
        for (name, hex) in SECTION_COLORS {
            let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
            let c = lizard_color::Color::from_hex(hex).unwrap_or(lizard_color::Color::WHITE);
            ui.painter().rect_filled(r, CornerRadius::same(9), crate::theme::doc_c32(ui.ctx(), c));
            let on = current.as_ref().and_then(|c| c.as_ref()).is_some_and(|h| h.eq_ignore_ascii_case(hex.trim_start_matches('#')));
            let stroke = if on { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.border) };
            ui.painter().rect_stroke(r, CornerRadius::same(9), stroke, egui::StrokeKind::Inside);
            let name = tr(name);
            crate::a11y::radio(&resp, name, on);
            if resp.lizard_tooltip(name).clicked() {
                app.set_props(&ids, json!({"fills": [hex]}));
            }
        }
    });
    row(ui, 28.0, |ui| {
        let hidden = sel.iter().all(|i| doc.node(*i).is_some_and(|n| n.hides_contents()));
        if widgets::checkbox(ui, hidden, tr("Hide contents")) {
            app.run("section.toggleContents", json!({"ids": ids, "hidden": !hidden}));
        }
    });
    ui.add_space(4.0);
    widgets::separator(ui);
}
