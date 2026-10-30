//! Design panel in vector edit mode: the selected points' position and dimensions (their
//! transform box), handle mirroring and the stroke's cap and join at them, or the
//! selected width point with the Variable width tool (in place of the layer's position and
//! dimensions).

use egui::{Color32, CornerRadius, Id, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use figcraft_doc::{Document, HandleMirroring};
use figcraft_engine::vectoredit::{self, VectorEdit};
use serde_json::{Value, json};

use crate::FigApp;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::icons::{AppearanceIcon, PositionIcon, VectorPointIcon};
use crate::primitives::{Button, ControlWidth, Segment, SegmentedControl};
use crate::right::{menu_popup, row, row_rect};
use crate::widgets::{self, Edit, FieldEdit, Lead, PAD};

/// Run a point command from a field: typed values are one step, scrubs one step when released.
fn point_field(app: &mut FigApp, cmd: &str, edit: Option<FieldEdit>, params: impl Fn(f64) -> Value) {
    match edit {
        Some(FieldEdit::Set(v)) => {
            app.run(cmd, params(v));
        }
        Some(FieldEdit::Scrub(v, done)) => {
            let _ = app.session.begin_interaction("Edit points");
            let _ = app.session.execute(cmd, &params(v));
            if done {
                let _ = app.session.commit_interaction();
            }
        }
        None => {}
    }
}

/// The Variable width tool's selected width point: where it is along the path and its width.
fn width_point(app: &mut FigApp, ui: &mut Ui, doc: &Document, ve: &VectorEdit) {
    widgets::section_header(ui, tr("Width point"), &[], false);
    let stroke = doc.node(ve.id).map(|n| &n.stroke);
    let sel = ve.width_point.and_then(|i| Some((i, *stroke?.profile()?.points.get(i)?)));
    let Some((i, p)) = sel else {
        let t = crate::theme::Tokens::get(ui.ctx());
        widgets::note(ui, tr("Drag the stroke to add a width point"), t.text_secondary);
        return;
    };
    let weight = stroke.map_or(1.0, |s| s.weight);
    let col = widgets::col_width(ui.available_width());
    let round = |v: f64| (v * 100.0).round() / 100.0;
    widgets::sublabels(ui, tr("Position"), Some(tr("Width")));
    row(ui, 32.0, |ui| {
        let e = widgets::number_field(ui, Id::new("wpt_t"), Lead::None, Some(round(p.t * 100.0)), "%", col, 1.0);
        point_field(app, "vector.edit.widthPoint", e, |v| json!({"index": i, "t": v / 100.0}));
        let e = widgets::number_field(ui, Id::new("wpt_w"), Lead::Icon(Icon::StrokeIcon, tr("Width")), Some(round(p.scale * weight)), "", col, 1.0);
        point_field(app, "vector.edit.widthPoint", e, |v| json!({"index": i, "width": v.max(0.0)}));
    });
    // Each side's reach from the centre line (Alt-drag a handle on the canvas for one side).
    let (l, r) = p.sides();
    widgets::sublabels(ui, tr("Left side"), Some(tr("Right side")));
    row(ui, 32.0, |ui| {
        for (side, scale, name) in [("left", l, tr("Left side width")), ("right", r, tr("Right side width"))] {
            let e = widgets::number_field(
                ui,
                Id::new(("wpt_side", side)),
                Lead::Icon(Icon::StrokeIcon, name),
                Some(round(scale * weight / 2.0)),
                "",
                col,
                1.0,
            );
            point_field(app, "vector.edit.widthPoint", e, |v| json!({"index": i, "side": side, "width": v.max(0.0)}));
        }
    });
    ui.add_space(8.0);
}

pub fn points(app: &mut FigApp, ui: &mut Ui, doc: &Document, ve: &VectorEdit) {
    if ve.tool == vectoredit::VTool::Width {
        width_point(app, ui, doc, ve);
        return;
    }
    let Some(net) = vectoredit::network(doc, ve.id) else { return };
    let pts = vectoredit::selected_points(net, ve);
    if pts.is_empty() {
        widgets::section_header(ui, if pts.len() > 1 { tr("Points") } else { tr("Point") }, &[], false);
        let t = crate::theme::Tokens::get(ui.ctx());
        widgets::note(ui, tr("Select points to edit their position, radius and mirroring"), t.text_secondary);
        return;
    }
    let col = widgets::col_width(ui.available_width());
    let pos = vectoredit::selection_position(doc, ve);
    let round = |v: f64| (v * 100.0).round() / 100.0;
    let align = [
        (PositionIcon::AlignLeft, tr("Align left"), "left"),
        (PositionIcon::AlignHCenter, tr("Align horizontal centers"), "hcenter"),
        (PositionIcon::AlignRight, tr("Align right"), "right"),
        (PositionIcon::AlignTop, tr("Align top"), "top"),
        (PositionIcon::AlignVCenter, tr("Align vertical centers"), "vcenter"),
        (PositionIcon::AlignBottom, tr("Align bottom"), "bottom"),
    ];
    let align_row = row_rect(ui, 32.0, |ui| {
        for group in [0..3, 3..6] {
            let items = align[group.clone()].iter().map(|(icon, label, _)| Segment::position_icon(*icon, label)).collect::<Vec<_>>();
            if let Some(index) = SegmentedControl::new(&items, None).width(ControlWidth::Fixed(col)).show(ui) {
                app.run("vector.edit.align", json!({"mode": align[group.start + index].2}));
            }
        }
    });
    let more = Button::vector_point_icon(VectorPointIcon::More, tr("More actions")).icon_size(16.0).show_at(
        ui,
        Id::new("vpt_more"),
        Rect::from_min_size(pos2(align_row.max.x - PAD - 24.0, align_row.min.y + 4.0), vec2(24.0, 24.0)),
    );
    point_options(app, pts.len(), &more);
    row(ui, 32.0, |ui| {
        let e = widgets::number_field(ui, Id::new("vpt_x"), Lead::Text("X"), pos.map(|p| round(p.x)), "", col, 1.0);
        point_field(app, "vector.edit.position", e, |v| json!({"x": v}));
        let e = widgets::number_field(ui, Id::new("vpt_y"), Lead::Text("Y"), pos.map(|p| round(p.y)), "", col, 1.0);
        point_field(app, "vector.edit.position", e, |v| json!({"y": v}));
    });
    if let Some(b) = vectoredit::transform::point_box(doc, ve).filter(|b| b.points.len() > 1) {
        widgets::sublabels(ui, tr("Dimensions"), None);
        row(ui, 32.0, |ui| {
            let e = widgets::number_field(ui, Id::new("vpt_w"), Lead::Text("W"), Some(round(b.width)), "", col, 1.0);
            point_field(app, "vector.edit.resize", e, |v| json!({"width": v.max(0.0)}));
            let e = widgets::number_field(ui, Id::new("vpt_h"), Lead::Text("H"), Some(round(b.height)), "", col, 1.0);
            point_field(app, "vector.edit.resize", e, |v| json!({"height": v.max(0.0)}));
        });
    }
    let mirror = same(pts.iter().map(|v| net.mirroring(*v)));
    row(ui, 32.0, |ui| {
        let modes = [HandleMirroring::None, HandleMirroring::Angle, HandleMirroring::AngleAndLength];
        let items = [
            Segment::vector_point_icon(VectorPointIcon::MirrorNone, tr("No mirroring")),
            Segment::vector_point_icon(VectorPointIcon::MirrorAngle, tr("Mirror angle")),
            Segment::vector_point_icon(VectorPointIcon::MirrorAngleAndLength, tr("Mirror angle and length")),
        ];
        let selected = mirror.and_then(|mode| modes.iter().position(|item| *item == mode));
        if let Some(index) = SegmentedControl::new(&items, selected).width(ControlWidth::Fixed(col * 2.0 + 8.0)).show(ui) {
            app.run("vector.edit.mirroring", json!({"mode": modes[index].id()}));
        }
    });
    let radius = crate::right::selected_point_radius(doc, ve).flatten();
    row(ui, 32.0, |ui| {
        let lead = Lead::AppearanceIcon(AppearanceIcon::CornerTopLeft, tr("Corner radius"));
        let edit = widgets::number_field_mixed(ui, Id::new("vpt_radius"), lead, radius, "", col, 1.0);
        match edit {
            Some(Edit::All(field)) => point_field(app, "vector.edit.cornerRadius", Some(field), |v| json!({"radius": v.max(0.0)})),
            Some(Edit::Each(widgets::Rel::Add(v))) => {
                app.run("vector.edit.cornerRadius", json!({"add": v}));
            }
            Some(Edit::Each(widgets::Rel::Mul(v))) => {
                app.run("vector.edit.cornerRadius", json!({"multiply": v}));
            }
            Some(Edit::Each(widgets::Rel::Div(v))) => {
                app.run("vector.edit.cornerRadius", json!({"divide": v}));
            }
            None => {}
        }
    });
    ui.add_space(8.0);
}

/// Arrange actions from the point panel's rightmost button.
fn point_options(app: &mut FigApp, selected_count: usize, more: &egui::Response) {
    menu_popup(more).width(320.0).show(|ui| {
        crate::primitives::MenuItem::new(tr("Tidy up"))
            .shortcut(crate::shortcuts::shortcut("arrange.tidy").unwrap_or(""))
            .enabled(false)
            .show_with_parts(ui, 28.0, |painter, rect, ink| arrange_icon(painter, rect, ink, 0), 0.0, |_, _, _| {});
        for (label, axis, command) in
            [("Distribute vertical spacing", 1, "distribute.vertical"), ("Distribute horizontal spacing", 2, "distribute.horizontal")]
        {
            if crate::primitives::MenuItem::new(tr(label))
                .shortcut(crate::shortcuts::shortcut(command).unwrap_or(""))
                .enabled(selected_count >= 3)
                .show_with_parts(ui, 28.0, |painter, rect, ink| arrange_icon(painter, rect, ink, axis), 0.0, |_, _, _| {})
                .clicked()
            {
                app.run(command, json!({}));
                ui.close();
            }
        }
    });
}

/// Small arrangement marks from the supplied menu reference; the shared menu primitive owns
/// layout, hit targets, hover and disabled states.
fn arrange_icon(painter: &egui::Painter, rect: Rect, ink: Color32, kind: u8) {
    let center = rect.center();
    let muted = ink.gamma_multiply(0.55);
    match kind {
        0 => {
            let box_rect = Rect::from_center_size(center, vec2(16.0, 16.0));
            painter.rect_stroke(box_rect, CornerRadius::same(2), Stroke::new(1.2, ink), StrokeKind::Inside);
            for y in [-4.0, 0.0, 4.0] {
                painter.rect_filled(Rect::from_center_size(pos2(center.x, center.y + y), vec2(9.0, 1.5)), CornerRadius::same(1), ink);
            }
        }
        1 => {
            for y in [-6.0, 6.0] {
                painter.line_segment([pos2(center.x - 8.0, center.y + y), pos2(center.x + 8.0, center.y + y)], Stroke::new(1.0, muted));
            }
            painter.rect_filled(Rect::from_center_size(center, vec2(9.0, 2.5)), CornerRadius::same(1), ink);
        }
        _ => {
            for x in [-6.0, 6.0] {
                painter.line_segment([pos2(center.x + x, center.y - 8.0), pos2(center.x + x, center.y + 8.0)], Stroke::new(1.0, muted));
            }
            painter.rect_filled(Rect::from_center_size(center, vec2(2.5, 9.0)), CornerRadius::same(1), ink);
        }
    }
}

/// The value every item shares, if they all share one.
fn same<T: PartialEq>(mut it: impl Iterator<Item = T>) -> Option<T> {
    let first = it.next()?;
    it.all(|x| x == first).then_some(first)
}
