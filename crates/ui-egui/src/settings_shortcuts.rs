//! The Keyboard shortcuts section in Settings: searchable keymap groups and platform keycaps.

use crate::primitives::TooltipResponseExt as _;
use egui::{Align2, CornerRadius, Sense, Ui, pos2, vec2};
use lizard_engine::keymap::{GESTURES, Group, KEYS, Platform};

use crate::LizardApp;
use crate::i18n::{self, tr};
use crate::primitives::{ControlWidth, SearchField, Select, SelectPosition, SelectVariant, Shortcut};
use crate::theme::{Tokens, regular};

/// Most keys a row shows (a command's first bindings; the rest stay in the menus' alternates).
const MAX_KEYS: usize = 2;
const ROW_H: f32 = 26.0;

/// One row: the action and its keys as written on this platform.
struct Row {
    label: std::borrow::Cow<'static, str>,
    keys: Vec<String>,
}

fn rows(group: Option<Group>, query: &str, platform: Platform) -> Vec<Row> {
    let q = query.trim().to_lowercase();
    // Labels match in the UI language and in English.
    let wanted = |g: Group, label: &str, english: &str| {
        group.is_none_or(|x| x == g) && (q.is_empty() || label.to_lowercase().contains(&q) || english.to_lowercase().contains(&q))
    };
    let mut out: Vec<Row> = KEYS
        .iter()
        .map(|b| (b, i18n::command(b.id, b.title())))
        .filter(|(b, label)| wanted(b.group, label, b.title()))
        .map(|(b, label)| {
            // Alternatives that read the same (⌘+ and ⇧⌘+) show once.
            let mut keys: Vec<String> = Vec::with_capacity(MAX_KEYS);
            for k in b.keys(platform) {
                let shown = lizard_engine::keymap::display(k, platform);
                if keys.len() < MAX_KEYS && !keys.contains(&shown) {
                    keys.push(shown);
                }
            }
            Row { label, keys }
        })
        .collect();
    out.extend(GESTURES.iter().filter(|(g, label, ..)| wanted(*g, tr(label), label)).map(|(_, label, mac, other)| Row {
        label: tr(label).into(),
        keys: vec![tr(if platform == Platform::Mac { mac } else { other }).to_string()],
    }));
    out
}

/// Paint `text` as a keycap ending at `right`; returns its left edge.
fn keycap(ui: &Ui, t: &Tokens, text: &str, right: f32, cy: f32) -> f32 {
    Shortcut::new(text).ink(t.text).paint_right(ui, right, cy)
}

fn row(ui: &mut Ui, t: &Tokens, row: &Row, width: f32) {
    let (r, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(4), t.hover);
    }
    let cy = r.center().y;
    let mut right = r.max.x - 6.0;
    for (i, k) in row.keys.iter().enumerate().rev() {
        right = keycap(ui, t, k, right, cy);
        if i > 0 {
            let or = ui.painter().layout_no_wrap(tr("or").into(), regular(10.0), t.text_tertiary);
            right -= 4.0 + or.size().x;
            ui.painter().galley(pos2(right, cy - or.size().y / 2.0), or, t.text_tertiary);
            right -= 4.0;
        }
    }
    let label_w = (right - r.min.x - 14.0).max(20.0);
    // Measured in the font it's painted in: a long translation ends with "…" before the keycaps and
    // shows in full on hover.
    let label = crate::widgets::elide_font(ui, &row.label, label_w, regular(12.0));
    if label != row.label {
        resp.lizard_tooltip(&*row.label);
    }
    ui.painter().text(pos2(r.min.x + 6.0, cy), Align2::LEFT_CENTER, label, regular(12.0), t.text);
}

pub fn show(app: &mut LizardApp, ui: &mut Ui, t: &Tokens) {
    let platform = crate::shortcuts::platform();
    ui.horizontal(|ui| {
        let groups = Group::ALL.map(|group| tr(group.label()));
        let selected = Group::ALL.iter().position(|group| *group == app.ui.shortcuts_tab).unwrap_or(0);
        if let Some(index) = Select::new(&groups, selected)
            .label(tr("Keyboard shortcuts"))
            .width(ControlWidth::Fixed(140.0))
            .position(SelectPosition::BottomLeft)
            .variant(SelectVariant::Ghost)
            .show(ui)
            && let Some(group) = Group::ALL.get(index)
        {
            app.ui.shortcuts_tab = *group;
        }
        ui.add_space(8.0);
        let response = SearchField::new(ui.id().with("settings_shortcuts_search"), tr("Search shortcuts"))
            .width(ControlWidth::Fill)
            .compact()
            .clearable()
            .show(ui, &mut app.ui.shortcuts_query);
        crate::widgets::select_all_on_focus(ui, &response, &app.ui.shortcuts_query);
    });
    ui.add_space(14.0);
    let searching = !app.ui.shortcuts_query.trim().is_empty();
    let list = rows((!searching).then_some(app.ui.shortcuts_tab), &app.ui.shortcuts_query, platform);
    if list.is_empty() {
        ui.label(egui::RichText::new(tr("No shortcuts match")).color(t.text_secondary).font(regular(12.0)));
        return;
    }
    for item in &list {
        row(ui, t, item, ui.available_width());
        ui.add_space(2.0);
    }
}
