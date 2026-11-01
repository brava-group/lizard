//! Component, component-property and instance sections of the Design panel.
//!
//! - An instance: its main component (with a swap menu), variant dropdowns, property values,
//!   the selected layer's overrides, and Reset / Detach / Go to main / Push changes.
//! - A component or component set: its properties (add, rename, default, delete), Add variant
//!   (a lone main component becomes a set) and, for a set, the variant property matrix.
//! - Several main components: Combine as variants.
//! - A layer inside a component: bind its visibility / text / instance to a property, or make a
//!   frame a slot.
//! - A slot inside an instance (and an instance's slot properties): fill it with components
//!   (preferred ones first), select it, reset it to the main's default content.

use egui::{Align2, Id, Rect, Sense, Ui, pos2, vec2};
use lizard_doc::*;
use lizard_engine::cmd::component::can_combine_variants;
use lizard_engine::components::{
    PROP_BINDING, default_variant, inside_instance, owning_component, owning_instance, slot_def, slot_edited, slots_of, variant_conflicts,
    variant_options, variant_props, variant_set,
};
use serde_json::{Value, json};

use crate::LizardApp;
use crate::i18n::{tr, tr_noop, trn};
use crate::icons::{self, Icon};
use crate::list_drag::ListDrag;
use crate::primitives::{Button, ButtonVariant, ControlWidth, FloatingPanel, FloatingPanelHeight, FloatingPanelWidth, MenuItem, PropertyController, PropertyControllerKind, Select, Switch, TextFieldWithPresets};
use crate::right::{menu_item, menu_popup, row, row_rect};
use crate::theme::{COMPONENT, Tokens, semibold};
use crate::widgets;

/// Entry point (called from the Design panel header for every selection).
pub fn instance_section(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    let [id] = sel else {
        ui.data_mut(|data| data.remove::<PropertyDraft>(property_draft_id()));
        combine_ui(app, ui, doc, sel);
        return;
    };
    if let Some(draft) = ui.data(|data| data.get_temp::<PropertyDraft>(property_draft_id()))
        && variant_set(doc, *id).unwrap_or(*id) != draft.owner
    {
        ui.data_mut(|data| data.remove::<PropertyDraft>(property_draft_id()));
    }
    let Some(n) = doc.node(*id) else { return };
    match &n.kind {
        NodeKind::Instance { .. } => instance_ui(app, ui, doc, n),
        NodeKind::ComponentSet { .. } => {
            properties_ui(app, ui, doc, n.id);
        }
        NodeKind::Component { .. } => {
            if let Some(set) = variant_set(doc, n.id) {
                variant_ui(app, ui, doc, n, set);
            } else {
                properties_ui(app, ui, doc, n.id);
            }
        }
        _ => {}
    }
    if owning_instance(doc, n.id).is_some()
        && let Some((name, preferred)) = slot_def(doc, n.id)
    {
        slot_section(app, ui, doc, n.id, name, preferred);
    }
    if !matches!(n.kind, NodeKind::Instance { .. }) && n.source.is_some() && owning_instance(doc, n.id).is_some() {
        overrides_ui(app, ui, n);
    }
    if !matches!(n.kind, NodeKind::Component { .. } | NodeKind::ComponentSet { .. })
        && !inside_instance(doc, n.id)
        && owning_component(doc, n.id).is_some()
    {
        bind_ui(app, ui, doc, n);
    }
}

/// Values of the selected variant. The component set owns the property definitions, so its
/// editable property list stays on the set itself.
fn variant_ui(app: &mut LizardApp, ui: &mut Ui, doc: &Document, variant: &Node, set: NodeId) {
    widgets::separator(ui);
    if widgets::section_header(ui, tr("Current variant"), &[(Icon::GoToComponentSet, tr("Go to component set"))], true).is_some() {
        app.run("select.set", json!({"ids": [set]}));
    }
    let current = variant_props(&variant.name);
    for (property, values) in variant_options(doc, set) {
        let value = current.get(&property).cloned().unwrap_or_default();
        row(ui, 32.0, |ui| {
            let width = (ui.available_width() - 8.0).max(0.0) / 2.0;
            let (name_rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
            let name_id = Id::new(("current_variant_name", variant.id, property.as_str()));
            if let Some(name) = widgets::text_field_inline(ui, name_id, &property, name_rect.shrink2(vec2(2.0, 1.0)), crate::theme::regular(11.0))
                && name.trim() != property
            {
                app.run("component.editProperty", json!({"id": set, "name": property, "newName": name}));
            }
            ui.add_space(8.0);
            let value_id = Id::new(("current_variant_value", variant.id, property.as_str()));
            let options: Vec<&str> = values.iter().map(String::as_str).collect();
            if let Some(next) = TextFieldWithPresets::new(value_id, &value, &options).rename_action().show(ui, ControlWidth::Fill) {
                app.run("component.setVariantValue", json!({"id": variant.id, "property": property, "value": next}));
            }
        });
    }
    ui.add_space(8.0);
}

/// Two or more main components selected: a button combines them into a component set.
fn combine_ui(app: &mut LizardApp, ui: &mut Ui, doc: &Document, sel: &[NodeId]) {
    if !can_combine_variants(doc, sel) {
        return;
    }
    row(ui, 32.0, |ui| {
        let label = crate::i18n::command("component.combineVariants", "Combine as variants");
        if crate::primitives::Button::new(&label)
            .variant(crate::primitives::ButtonVariant::Input)
            .align(crate::primitives::ButtonAlign::Start)
            .width(crate::primitives::ControlWidth::Fill)
            .leading(Icon::ComponentSet)
            .show(ui)
            .clicked()
        {
            app.run("component.combineVariants", json!({"ids": sel}));
        }
    });
}

/// Local components (and component sets, not their variants), for swap menus: (id, label), page
/// by page. Listed once per document revision (a file with thousands of layers isn't walked every
/// frame).
fn local_components(app: &LizardApp, ui: &Ui, doc: &Document) -> std::sync::Arc<Vec<(NodeId, String)>> {
    type Cached = ((u64, u64), std::sync::Arc<Vec<(NodeId, String)>>);
    let key = app.session.ds().map(|d| (d.uid, d.revision)).unwrap_or_default();
    let id = Id::new("component_panel_local_components");
    if let Some((k, list)) = ui.data(|d| d.get_temp::<Cached>(id))
        && k == key
    {
        return list;
    }
    let list = std::sync::Arc::new(list_components(doc));
    ui.data_mut(|d| d.insert_temp::<Cached>(id, (key, list.clone())));
    list
}

fn list_components(doc: &Document) -> Vec<(NodeId, String)> {
    let mut out: Vec<(NodeId, String)> = Vec::new();
    for page in &doc.pages {
        for d in doc.subtree(*page) {
            let Some(n) = doc.node(d) else { continue };
            let is = match n.kind {
                NodeKind::Component { .. } => variant_set(doc, d).is_none(),
                NodeKind::ComponentSet { .. } => true,
                _ => false,
            };
            if is && !inside_instance(doc, d) {
                out.push((d, n.name.clone()));
            }
        }
    }
    out
}

/// A section header row with a title and a "+" button; returns the button response.
fn header_with_plus(ui: &mut Ui, title: &str, tip: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(
        egui::pos2(rect.min.x + widgets::PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        crate::theme::property_section_title(),
        t.text,
    );
    let button = widgets::section_action_rect(rect, 0);
    crate::primitives::Button::icon(Icon::Plus, tip)
        .icon_size(14.0)
        .show_at(ui, ui.id().with(("component_properties_add", title)), button)
}

fn type_kind(d: &PropDef) -> PropertyControllerKind {
    match d {
        PropDef::Boolean { .. } => PropertyControllerKind::Boolean,
        PropDef::Text { .. } => PropertyControllerKind::Text,
        PropDef::InstanceSwap { .. } => PropertyControllerKind::InstanceSwap,
        PropDef::Variant { .. } => PropertyControllerKind::Variant,
        PropDef::Slot { .. } => PropertyControllerKind::Slot,
    }
}

/// A selected slot inside an instance: its fill menu.
fn slot_section(app: &mut LizardApp, ui: &mut Ui, doc: &Document, slot: NodeId, name: &str, preferred: &[NodeId]) {
    let t = Tokens::get(ui.ctx());
    let comps = local_components(app, ui, doc);
    widgets::separator(ui);
    row(ui, 36.0, |ui| {
        ui.label(egui::RichText::new(tr("Slot")).font(semibold(11.0)).color(t.text));
    });
    row(ui, 32.0, |ui| {
        prop_label(ui, name, PropertyControllerKind::Slot);
        let w = ui.available_width().max(60.0);
        slot_fill(app, ui, doc, slot, preferred, &comps, w);
    });
}

/// The fill menu of slot `slot` inside an instance: its preferred components first, then every
/// local component; select the slot; reset it to the main's default content.
fn slot_fill(app: &mut LizardApp, ui: &mut Ui, doc: &Document, slot: NodeId, preferred: &[NodeId], comps: &[(NodeId, String)], w: f32) {
    let t = Tokens::get(ui.ctx());
    let edited = slot_edited(doc, slot);
    let label = match (edited, doc.node(slot).map_or(0, |n| n.children.len())) {
        (false, 0) => tr("Empty").to_string(),
        (false, _) => tr("Default content").to_string(),
        (true, k) => trn("{n} layers", k, &[]),
    };
    let resp = crate::primitives::Select::trigger(ui, &label, w, Some(Icon::Slot)).on_hover_text(tr("Add content to this slot"));
    menu_popup(&resp).width(w.max(200.0)).show(|ui| {
        let insert = |app: &mut LizardApp, c: NodeId| app.run("instance.slotInsert", json!({"id": slot, "component": c.0}));
        let heading = |ui: &mut Ui, text: &str| ui.label(egui::RichText::new(text).font(semibold(11.0)).color(t.text_secondary));
        if !preferred.is_empty() {
            heading(ui, tr("Preferred"));
            for c in preferred.iter().filter(|c| doc.contains(**c)) {
                let name = doc.node(*c).map(|n| n.name.clone()).unwrap_or_default();
                if menu_item(ui, &name, "") {
                    insert(app, *c);
                    ui.close();
                }
            }
        }
        heading(ui, tr("All components"));
        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
            // Never a component the slot is part of.
            for (c, name) in comps.iter().filter(|(c, _)| !doc.is_ancestor(*c, slot)) {
                if menu_item(ui, name, "") {
                    insert(app, *c);
                    ui.close();
                }
            }
        });
        if menu_item(ui, tr("Select slot"), "") {
            app.run("select.set", json!({"ids": [slot.0]}));
            ui.close();
        }
        if edited && menu_item(ui, tr("Reset slot"), "") {
            app.run("instance.resetSlot", json!({"id": slot.0}));
            ui.close();
        }
    });
}

fn prop_defs(doc: &Document, c: NodeId) -> std::collections::BTreeMap<String, PropDef> {
    match doc.node(c).map(|n| &n.kind) {
        Some(NodeKind::Component { props, .. }) => props.clone(),
        _ => Default::default(),
    }
}

fn property_is_used(doc: &Document, owner: NodeId, name: &str) -> bool {
    let components: Vec<NodeId> = match doc.node(owner).map(|node| &node.kind) {
        Some(NodeKind::ComponentSet { .. }) => doc
            .node(owner)
            .map(|set| set.children.iter().copied().filter(|id| matches!(doc.node(*id).map(|node| &node.kind), Some(NodeKind::Component { .. }))).collect())
            .unwrap_or_default(),
        _ => vec![owner],
    };
    components.into_iter().any(|component| {
        doc.subtree(component)
            .into_iter()
            .filter_map(|id| doc.node(id))
            .any(|node| node.prop_refs.values().any(|property| property == name))
    })
}

fn property_menu_row(ui: &mut Ui, label: &str, kind: &str, enabled: bool) -> egui::Response {
    MenuItem::new(label).enabled(enabled).show_with_parts(
        ui,
        20.0,
        |painter, rect, ink| {
            if let Some(kind) = PropertyControllerKind::from_code(kind) {
                kind.paint(painter, rect, ink);
            }
        },
        0.0,
        |_, _, _| {},
    )
}

#[derive(Clone)]
struct PropertyDraft {
    owner: NodeId,
    kind: &'static str,
    name: String,
    text: String,
    boolean: bool,
    instance: Option<NodeId>,
    preferred: Vec<NodeId>,
}

fn property_draft_id() -> Id {
    Id::new("component_property_create_draft")
}

/// Menu shared by the Properties add button and the component header settings action.
pub(crate) fn add_property_menu(app: &mut LizardApp, ui: &mut Ui, doc: &Document, owner: NodeId) {
    let first = if matches!(doc.node(owner).map(|n| &n.kind), Some(NodeKind::ComponentSet { .. })) {
        doc.node(owner).and_then(|set| set.children.iter().copied().find(|id| matches!(doc.node(*id).map(|n| &n.kind), Some(NodeKind::Component { .. }))))
    } else {
        Some(owner)
    };
    let defs = first.map(|id| prop_defs(doc, id)).unwrap_or_default();
    let variants = if matches!(doc.node(owner).map(|n| &n.kind), Some(NodeKind::ComponentSet { .. })) {
        variant_options(doc, owner)
    } else {
        Vec::new()
    };
    let existing: Vec<String> = defs.keys().cloned().chain(variants.iter().map(|(key, _)| key.clone())).collect();
    let fresh = (1..).map(|index| format!("Property {index}")).find(|name| !existing.contains(name)).unwrap_or_else(|| "Property".into());
    let comps = local_components(app, ui, doc);
    let swap_default = comps.iter().map(|(id, _)| *id).find(|id| *id != owner && first != Some(*id));
    ui.add_space(4.0);
    crate::primitives::Menu::status(ui, tr("Create property"));
    let items: [(&str, &str, Value); 4] = [
        (tr("Variant"), "variant", json!("Default")),
        (tr("Text"), "text", json!("Text")),
        (tr("Boolean"), "boolean", json!(true)),
        (tr("Instance swap"), "instanceSwap", swap_default.map(|id| json!(id.0)).unwrap_or(Value::Null)),
    ];
    for (label, ty, default) in items {
        if property_menu_row(ui, label, ty, true).clicked() {
            if ty == "variant" {
                app.run("component.addProperty", json!({"id": owner, "name": fresh, "type": ty, "default": default}));
            } else {
                let base = match ty {
                    "text" => tr("Text"),
                    "boolean" => tr("Boolean"),
                    _ => tr("Instance"),
                };
                let name = if existing.contains(&base.to_string()) { fresh.clone() } else { base.to_string() };
                ui.data_mut(|data| {
                    data.insert_temp(
                        property_draft_id(),
                        PropertyDraft { owner, kind: ty, name, text: String::new(), boolean: true, instance: None, preferred: Vec::new() },
                    )
                });
            }
            ui.close();
        }
    }
    if property_menu_row(ui, tr("Slot"), "slot", first.is_some()).clicked() {
        if let Some(component) = first {
            app.run(
                "command.batch",
                json!({
                    "label": "Create slot property",
                    "commands": [
                        {"command": "node.create", "as": "slot", "params": {"type": "frame", "parent": component, "name": fresh}},
                        {"command": "component.makeSlot", "params": {"id": {"$ref": "slot"}, "name": fresh}},
                        {"command": "select.set", "params": {"ids": [owner]}}
                    ]
                }),
            );
        }
        ui.close();
    }
}

fn property_form_label(ui: &mut Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(110.0, 24.0), Sense::hover());
    ui.painter().text(pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, label, crate::theme::regular(11.0), Tokens::get(ui.ctx()).text_secondary);
}

fn property_form_field(ui: &mut Ui, id: Id, label: &str, value: &str) -> Option<String> {
    let mut changed = None;
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        property_form_label(ui, label);
        changed = widgets::text_field_named(ui, id, value, 178.0, label);
        ui.add_space(16.0);
    });
    changed
}

fn property_create_dialog(app: &mut LizardApp, ui: &mut Ui, doc: &Document, owner: NodeId) {
    let ctx = ui.ctx().clone();
    let Some(mut draft) = ctx.data(|data| data.get_temp::<PropertyDraft>(property_draft_id())) else { return };
    if draft.owner != owner {
        ctx.data_mut(|data| data.remove::<PropertyDraft>(property_draft_id()));
        return;
    }
    let first = if matches!(doc.node(owner).map(|node| &node.kind), Some(NodeKind::ComponentSet { .. })) {
        doc.node(owner).and_then(|set| set.children.iter().copied().find(|id| matches!(doc.node(*id).map(|node| &node.kind), Some(NodeKind::Component { .. }))))
    } else {
        Some(owner)
    };
    let defs = first.map(|id| prop_defs(doc, id)).unwrap_or_default();
    let variants = if first == Some(owner) { Vec::new() } else { variant_options(doc, owner) };
    let comps = local_components(app, ui, doc);
    let panel_id = Id::new(("component_property_create", owner));
    let mut open = true;
    let mut close = false;
    let mut create = false;
    FloatingPanel::new(panel_id, pos2(0.0, ui.cursor().min.y))
        .width(FloatingPanelWidth::Fixed(320.0))
        .height(FloatingPanelHeight::Fixed(if draft.kind == "instanceSwap" { 252.0 } else { 202.0 }))
        .header_height(42.0)
        .anchor_to_properties()
        .open(&mut open)
        .show_with_header(&ctx, |ui, header| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            close = FloatingPanel::title_header(ui, header, tr("Create property"), panel_id.with("close"));
            ui.add_space(16.0);
            if let Some(name) = property_form_field(ui, panel_id.with("name"), tr("Name"), &draft.name) {
                draft.name = name;
            }
            ui.add_space(10.0);
            match draft.kind {
                "text" => {
                    if let Some(value) = property_form_field(ui, panel_id.with("value"), tr("Value"), &draft.text) {
                        draft.text = value;
                    }
                }
                "boolean" => {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        property_form_label(ui, tr("Value"));
                        let (rect, _) = ui.allocate_exact_size(vec2(178.0, 24.0), Sense::hover());
                        if Switch::new(panel_id.with("boolean"), draft.boolean, if draft.boolean { tr("True") } else { tr("False") }).show_at(ui, rect).clicked() {
                            draft.boolean = !draft.boolean;
                        }
                    });
                }
                "instanceSwap" => {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        property_form_label(ui, tr("Value"));
                        let name = draft.instance.and_then(|id| doc.node(id).map(|node| node.name.as_str())).unwrap_or(tr("Choose instance"));
                        let response = Select::trigger(ui, name, 178.0, Some(Icon::Instance));
                        menu_popup(&response).width(220.0).show(|ui| {
                            for (id, name) in comps.iter().filter(|(id, _)| *id != owner && first != Some(*id)) {
                                if MenuItem::new(name).leading(Icon::Instance).show(ui).clicked() {
                                    draft.instance = Some(*id);
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.add_space(16.0);
                    widgets::separator(ui);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        let (label, _) = ui.allocate_exact_size(vec2(264.0, 24.0), Sense::hover());
                        ui.painter().text(label.left_center(), Align2::LEFT_CENTER, tr("Preferred instances"), crate::theme::regular(11.0), Tokens::get(ui.ctx()).text_secondary);
                        let plus = Button::icon(Icon::Plus, tr("Add preferred instance")).show(ui);
                        menu_popup(&plus).width(220.0).show(|ui| {
                            for (id, name) in comps.iter().filter(|(id, _)| *id != owner && first != Some(*id)) {
                                if MenuItem::new(name).checked(draft.preferred.contains(id)).show(ui).clicked() {
                                    if let Some(index) = draft.preferred.iter().position(|candidate| candidate == id) {
                                        draft.preferred.remove(index);
                                    } else {
                                        draft.preferred.push(*id);
                                    }
                                }
                            }
                        });
                    });
                }
                _ => {}
            }
            ui.add_space(20.0);
            widgets::separator(ui);
            ui.add_space(14.0);
            let valid_name = !draft.name.trim().is_empty()
                && !defs.contains_key(draft.name.trim())
                && !variants.iter().any(|(name, _)| name == draft.name.trim());
            let enabled = valid_name && (draft.kind != "instanceSwap" || draft.instance.is_some());
            let (footer, _) = ui.allocate_exact_size(vec2(320.0, 32.0), Sense::hover());
            let button = Rect::from_min_size(pos2(footer.right() - 166.0, footer.top() + 4.0), vec2(150.0, 24.0));
            create = Button::new(tr("Create property"))
                .variant(ButtonVariant::Primary)
                .enabled(enabled)
                .show_at(ui, panel_id.with("create"), button)
                .clicked();
            ui.add_space(16.0);
        });
    if create {
        let default = match draft.kind {
            "text" => json!(draft.text),
            "boolean" => json!(draft.boolean),
            "instanceSwap" => draft.instance.map(|id| json!(id.0)).unwrap_or(Value::Null),
            _ => Value::Null,
        };
        let preferred: Vec<u64> = draft.preferred.iter().map(|id| id.0).collect();
        if app.run("component.addProperty", json!({"id": owner, "name": draft.name.trim(), "type": draft.kind, "default": default, "preferred": preferred})).is_some() {
            close = true;
        }
    }
    if close || !open || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|data| data.remove::<PropertyDraft>(property_draft_id()));
        FloatingPanel::dismiss(&ctx, panel_id);
    } else {
        ctx.data_mut(|data| data.insert_temp(property_draft_id(), draft));
    }
}

/// Properties of a component or component set.
fn properties_ui(app: &mut LizardApp, ui: &mut Ui, doc: &Document, owner: NodeId) {
    let t = Tokens::get(ui.ctx());
    let is_set = matches!(doc.node(owner).map(|n| &n.kind), Some(NodeKind::ComponentSet { .. }));
    // Shared (non-variant) properties live on every variant; read them from the first.
    let first = if is_set {
        doc.node(owner).and_then(|s| s.children.iter().copied().find(|c| matches!(doc.node(*c).map(|n| &n.kind), Some(NodeKind::Component { .. }))))
    } else {
        Some(owner)
    };
    let defs = first.map(|c| prop_defs(doc, c)).unwrap_or_default();
    let variants = if is_set { variant_options(doc, owner) } else { Vec::new() };
    let comps = local_components(app, ui, doc);
    widgets::separator(ui);
    let plus = header_with_plus(ui, tr("Properties"), tr("Create component property"));
    menu_popup(&plus).width(220.0).show(|ui| add_property_menu(app, ui, doc, owner));
    property_create_dialog(app, ui, doc, owner);
    if is_set && !variant_conflicts(doc, owner).is_empty() {
        widgets::note(ui, tr("Some variants have the same property values: give each one a unique combination"), t.warning);
    }
    let defaults = if is_set {
        default_variant(doc, owner).and_then(|d| doc.node(d)).map(|n| variant_props(&n.name)).unwrap_or_default()
    } else {
        Default::default()
    };
    // Variant properties have a compact summary; clicking one exposes its name and value matrix.
    let mut drag = ListDrag::in_order(ui, ("variant_props", owner));
    for (i, (k, vals)) in variants.iter().enumerate() {
        let def = defaults.get(k).or(vals.first()).cloned().unwrap_or_default();
        let expanded_id = Id::new(("variant_property_expanded", owner, k.as_str()));
        let mut expanded = ui.data(|data| data.get_temp::<bool>(expanded_id)).unwrap_or(false);
        let summary = PropertyController::new(PropertyControllerKind::Variant, k).value(&def).selected(expanded).show(ui);
        if let Some(name) = summary.renamed.as_deref() {
            app.run("component.editProperty", json!({"id": owner, "name": k, "newName": name}));
        }
        if summary.delete_clicked {
            app.run("component.deleteProperty", json!({"id": owner, "name": k}));
            expanded = false;
        }
        if summary.settings_clicked {
            expanded = !expanded;
            ui.data_mut(|data| data.insert_temp(expanded_id, expanded));
        }
        if expanded {
            row_rect(ui, 32.0, |ui| {
                let (ir, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), ir, Icon::ComponentSet, COMPONENT);
                if let Some(nn) = widgets::text_field(ui, Id::new(("vprop_name", owner, k.as_str())), k, 110.0, tr("Name")) {
                    app.run("component.editProperty", json!({"id": owner, "name": k, "newName": nn}));
                }
                let resp = crate::primitives::Select::trigger(ui, &def, 70.0, None).on_hover_text(tr("Default value (the top-left variant)"));
                menu_popup(&resp).show(|ui| {
                    for v in vals {
                        if menu_item(ui, v, "") {
                            app.run("component.editProperty", json!({"id": owner, "name": k, "default": v}));
                            ui.close();
                        }
                    }
                });
                if icons::button(ui, Icon::Minus, 22.0, false, tr("Delete property")).clicked() {
                    app.run("component.deleteProperty", json!({"id": owner, "name": k}));
                }
            });
            // Values: rename inline; "+" adds a variant with a new value.
            let mut vi = 0;
            while vi < vals.len() {
                let chunk: Vec<String> = vals.iter().skip(vi).take(3).cloned().collect();
                let last = vi + chunk.len() >= vals.len();
                row(ui, 30.0, |ui| {
                    ui.add_space(22.0);
                    for v in &chunk {
                        if let Some(nv) = widgets::text_field(ui, Id::new(("vprop_val", owner, k.as_str(), v.as_str())), v, 62.0, "")
                            && nv != *v
                        {
                            app.run("component.editProperty", json!({"id": owner, "name": k, "value": v, "newValue": nv}));
                        }
                    }
                    if last && icons::button(ui, Icon::Plus, 22.0, false, tr("Add variant with a new value")).clicked() {
                        let nv = (vals.len() + 1..).map(|n| format!("Value {n}")).find(|n| !vals.contains(n)).unwrap_or_default();
                        app.run("component.addVariant", json!({"id": owner, "property": k, "value": nv}));
                    }
                });
                vi += 3;
            }
        }
        drag.row_by(&summary.row, i, summary.row.rect.with_max_y(ui.cursor().min.y.max(summary.row.rect.max.y)), || k.clone());
    }
    if let Some((from, to)) = drag.finish(ui)
        && let Some((k, _)) = variants.get(from)
    {
        app.run("component.editProperty", json!({"id": owner, "name": k, "index": to}));
    }
    // Shared properties follow the same type order as the creation menu.
    let mut shared: Vec<_> = defs.iter().filter(|(_, def)| !matches!(def, PropDef::Variant { .. })).collect();
    let has_properties = !variants.is_empty() || !shared.is_empty();
    shared.sort_by_key(|(name, def)| {
        let order = match def {
            PropDef::Text { .. } => 0,
            PropDef::Boolean { .. } => 1,
            PropDef::InstanceSwap { .. } => 2,
            PropDef::Slot { .. } => 3,
            PropDef::Variant { .. } => 4,
        };
        (order, *name)
    });
    for (k, d) in shared {
        let kind = type_kind(d);
        let value = match d {
            PropDef::Text { default } => Some(default.clone()),
            PropDef::Boolean { default } => Some(tr(if *default { "True" } else { "False" }).to_string()),
            PropDef::InstanceSwap { default, .. } => Some(doc.node(*default).map(|node| node.name.clone()).unwrap_or_else(|| tr("Missing").to_string())),
            PropDef::Slot { .. } | PropDef::Variant { .. } => None,
        };
        let expanded_id = Id::new(("component_property_expanded", owner, k.as_str()));
        let mut expanded = ui.data(|data| data.get_temp::<bool>(expanded_id)).unwrap_or(false);
        let mut controller = PropertyController::new(kind, k).unused(!property_is_used(doc, owner, k)).selected(expanded);
        if let Some(value) = value.as_deref() {
            controller = controller.value(value);
        }
        let summary = controller.show(ui);
        if let Some(name) = summary.renamed.as_deref() {
            app.run("component.editProperty", json!({"id": owner, "name": k, "newName": name}));
        }
        if summary.delete_clicked {
            app.run("component.deleteProperty", json!({"id": owner, "name": k}));
            continue;
        }
        if summary.settings_clicked {
            expanded = !expanded;
            ui.data_mut(|data| data.insert_temp(expanded_id, expanded));
        }
        if !expanded {
            continue;
        }
        row(ui, 32.0, |ui| {
            let (ir, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            type_kind(d).paint(ui.painter(), ir, COMPONENT);
            if let Some(nn) = widgets::text_field(ui, Id::new(("prop_name", owner, k.as_str())), k, 96.0, tr("Name")) {
                app.run("component.editProperty", json!({"id": owner, "name": k, "newName": nn}));
            }
            match d {
                PropDef::Boolean { default } => {
                    if widgets::checkbox(ui, *default, if *default { tr("True") } else { tr("False") }) {
                        app.run("component.editProperty", json!({"id": owner, "name": k, "default": !default}));
                    }
                }
                PropDef::Text { default } => {
                    if let Some(v) = widgets::text_field(ui, Id::new(("prop_def", owner, k.as_str())), default, 84.0, tr("Default")) {
                        app.run("component.editProperty", json!({"id": owner, "name": k, "default": v}));
                    }
                }
                PropDef::InstanceSwap { default, .. } => {
                    let label = doc.node(*default).map(|n| n.name.clone()).unwrap_or_else(|| tr("Missing").into());
                    let resp = crate::primitives::Select::trigger(ui, &label, 84.0, None);
                    menu_popup(&resp).show(|ui| {
                        for (c, name) in comps.iter() {
                            if menu_item(ui, name, "") {
                                app.run("component.editProperty", json!({"id": owner, "name": k, "default": c.0}));
                                ui.close();
                            }
                        }
                    });
                }
                PropDef::Variant { .. } | PropDef::Slot { .. } => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, Icon::Minus, 22.0, false, tr("Delete property")).clicked() {
                    app.run("component.deleteProperty", json!({"id": owner, "name": k}));
                }
            });
        });
        // An instance swap or slot property's preferred components, on a row of their own.
        if let PropDef::Slot { preferred } | PropDef::InstanceSwap { preferred, .. } = d {
            let tip = if matches!(d, PropDef::Slot { .. }) {
                tr("Offered first when filling the slot (any content is allowed)")
            } else {
                tr("Offered first when swapping the instance (any component is allowed)")
            };
            row(ui, 30.0, |ui| {
                ui.add_space(22.0);
                let label = match preferred.len() {
                    0 => tr("No preferred components").to_string(),
                    k => trn("{n} preferred components", k, &[]),
                };
                let w = (ui.available_width() - 8.0).max(60.0);
                let resp = crate::primitives::Select::trigger(ui, &label, w, None).on_hover_text(tip);
                menu_popup(&resp).width(w.max(200.0)).show(|ui| {
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for (c, name) in comps.iter().filter(|(c, _)| *c != owner && first != Some(*c)) {
                            let on = preferred.contains(c);
                            if menu_item(ui, &format!("{}{name}", if on { "✓ " } else { "" }), "") {
                                let next: Vec<u64> = preferred.iter().filter(|p| *p != c).map(|p| p.0).chain((!on).then_some(c.0)).collect();
                                app.run("component.editProperty", json!({"id": owner, "name": k, "preferred": next}));
                            }
                        }
                    });
                });
            });
        }
    }
    if has_properties {
        ui.add_space(12.0);
    }
}

/// Bind a layer inside a component to a component property.
fn bind_ui(app: &mut LizardApp, ui: &mut Ui, doc: &Document, n: &Node) {
    let t = Tokens::get(ui.ctx());
    let Some(comp) = owning_component(doc, n.id) else { return };
    let defs = prop_defs(doc, comp);
    // The English label also names a new property (document content); it shows translated.
    let mut fields: Vec<(&str, &str, &str)> = vec![("visible", tr_noop("Visibility"), "boolean")];
    if n.kind.text().is_some() {
        fields.push(("characters", tr_noop("Text"), "text"));
    }
    if matches!(n.kind, NodeKind::Instance { .. }) {
        fields.push(("mainComponent", tr_noop("Instance"), "instanceSwap"));
    }
    if matches!(n.kind, NodeKind::Frame) {
        fields.push(("slot", tr_noop("Slot"), "slot"));
    }
    widgets::separator(ui);
    row(ui, 36.0, |ui| {
        ui.label(egui::RichText::new(tr("Component property")).font(semibold(11.0)).color(t.text));
    });
    for (field, label, ty) in fields {
        let matching: Vec<&String> = defs
            .iter()
            .filter(|(_, d)| {
                matches!(
                    (ty, d),
                    ("boolean", PropDef::Boolean { .. })
                        | ("text", PropDef::Text { .. })
                        | ("instanceSwap", PropDef::InstanceSwap { .. })
                        | ("slot", PropDef::Slot { .. })
                )
            })
            .map(|(k, _)| k)
            .collect();
        let bound = n.prop_refs.get(field).cloned();
        row(ui, 32.0, |ui| {
            let (ir, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            let icon = match field {
                "visible" => Icon::Eye,
                "characters" => Icon::Text,
                "slot" => Icon::Slot,
                _ => Icon::Instance,
            };
            icons::paint(ui.painter(), ir, icon, if bound.is_some() { COMPONENT } else { t.icon_secondary });
            widgets::label(ui, tr(label), t.text_secondary);
            let shown = bound.clone().unwrap_or_else(|| tr("Not bound").into());
            let resp = crate::primitives::Select::trigger(ui, &shown, 120.0, None).on_hover_text(tr("Apply a component property"));
            menu_popup(&resp).width(180.0).show(|ui| {
                for k in &matching {
                    if menu_item(ui, k, "") {
                        app.run("component.bindProperty", json!({"layer": n.id, "property": k, "field": field}));
                        ui.close();
                    }
                }
                if ty == "slot" {
                    if bound.is_none() && menu_item(ui, tr("Make slot"), "") {
                        // Named after the frame.
                        app.run("component.makeSlot", json!({"id": n.id}));
                        ui.close();
                    }
                } else if menu_item(ui, tr("Create property…"), "") {
                    let existing: Vec<String> = defs.keys().cloned().collect();
                    let name = (1..)
                        .map(|k| if k == 1 { label.to_string() } else { format!("{label} {k}") })
                        .find(|x| !existing.contains(x))
                        .unwrap_or_default();
                    app.run("component.addProperty", json!({"id": n.id, "name": name, "type": ty}));
                    ui.close();
                }
                if ty == "slot" {
                    if bound.is_some() && menu_item(ui, tr("Remove slot"), "") {
                        app.run("component.removeSlot", json!({"id": n.id}));
                        ui.close();
                    }
                } else if bound.is_some() && menu_item(ui, tr("Detach property"), "") {
                    app.run("component.bindProperty", json!({"layer": n.id, "property": null, "field": field}));
                    ui.close();
                }
            });
        });
    }
}

/// Overridden property groups of a layer inside an instance, each with a reset button.
fn overrides_ui(app: &mut LizardApp, ui: &mut Ui, n: &Node) {
    let t = Tokens::get(ui.ctx());
    let groups: Vec<String> = n.overrides.iter().filter(|g| lizard_engine::components::OVERRIDE_GROUPS.contains(&g.as_str())).cloned().collect();
    if groups.is_empty() {
        return;
    }
    widgets::separator(ui);
    row(ui, 36.0, |ui| {
        ui.label(egui::RichText::new(tr("Overrides")).font(semibold(11.0)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icons::button(ui, Icon::Reset, 24.0, false, tr("Reset all overrides on this layer")).clicked() {
                app.run("component.resetOverride", json!({"id": n.id}));
            }
        });
    });
    for g in groups {
        row(ui, 26.0, |ui| {
            widgets::label(ui, &group_label(&g), t.text_secondary);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, Icon::Close, 20.0, false, tr("Reset this override")).clicked() {
                    app.run("component.resetOverride", json!({"id": n.id, "group": g}));
                }
            });
        });
    }
}

fn group_label(g: &str) -> String {
    match g {
        "fills" => tr("Fill"),
        "strokes" => tr("Stroke"),
        "effects" => tr("Effects"),
        "opacity" => tr("Opacity & blend"),
        "visible" => tr("Visibility"),
        "name" => tr("Layer name"),
        "corners" => tr("Corner radius"),
        "characters" => tr("Text content"),
        "textStyle" => tr("Text style"),
        "padding" => tr("Padding"),
        "gap" => tr("Gap"),
        "alignment" => tr("Alignment"),
        "grids" => tr("Layout grids"),
        "exports" => tr("Export settings"),
        "interactions" => tr("Interactions"),
        "size" => tr("Size"),
        "resizing" => tr("Resizing"),
        "position" => tr("Position"),
        "component" => tr("Swapped instance"),
        "props" => tr("Instance properties"),
        "slot" => tr("Slot content"),
        other => other,
    }
    .to_string()
}

/// The instance section: main component + swap, variants, properties, actions.
fn instance_ui(app: &mut LizardApp, ui: &mut Ui, doc: &Document, n: &Node) {
    let t = Tokens::get(ui.ctx());
    let NodeKind::Instance { component, .. } = &n.kind else { return };
    let id = n.id;
    let missing = !doc.contains(*component);
    let set = variant_set(doc, *component);
    let comp_name = set.or(Some(*component)).and_then(|c| doc.node(c)).map(|c| c.name.clone()).unwrap_or_else(|| tr("Main component deleted").into());
    let comps = local_components(app, ui, doc);
    let nested = inside_instance(doc, id);
    widgets::separator(ui);
    row(ui, 36.0, |ui| {
        ui.label(egui::RichText::new(if nested { tr("Nested instance") } else { tr("Instance") }).font(semibold(11.0)).color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let more = icons::button(ui, Icon::Dots, 24.0, false, tr("More instance actions"));
            menu_popup(&more).width(220.0).show(|ui| {
                if !missing && menu_item(ui, tr("Go to main component"), "") {
                    app.run("component.goToMain", json!({"id": id}));
                    ui.close();
                }
                if !missing && menu_item(ui, tr("Push changes to main component"), "") {
                    app.run("component.pushChanges", json!({"id": id}));
                    ui.close();
                }
                if missing && !nested && menu_item(ui, tr("Restore main component"), "") {
                    app.run("component.restoreMain", json!({"id": id}));
                    ui.close();
                }
                if menu_item(ui, tr("Reset all changes"), "") {
                    app.run("component.resetInstance", json!({"ids": [id]}));
                    ui.close();
                }
                if !nested && menu_item(ui, tr("Detach instance"), "Alt+Cmd+B") {
                    app.run("component.detach", json!({"ids": [id]}));
                    ui.close();
                }
            });
            if !nested && icons::button(ui, Icon::Detach, 24.0, false, crate::shortcuts::tip(tr("Detach instance"), "component.detach")).clicked() {
                app.run("component.detach", json!({"ids": [id]}));
            }
            if icons::button(ui, Icon::Reset, 24.0, false, tr("Reset all changes")).clicked() {
                app.run("component.resetInstance", json!({"ids": [id]}));
            }
        });
    });
    // Main component with a swap menu, and "go to main".
    row(ui, 32.0, |ui| {
        let w = ui.available_width().max(80.0);
        let resp = crate::primitives::Select::trigger(ui, &comp_name, w, Some(Icon::Instance)).on_hover_text(tr("Swap instance"));
        menu_popup(&resp).width(w.max(200.0)).show(|ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                let mut page = None;
                for (c, name) in comps.iter() {
                    let pg = doc.page_of(*c);
                    if pg != page {
                        page = pg;
                        let pname = pg.and_then(|p| doc.node(p)).map(|p| p.name.clone()).unwrap_or_default();
                        ui.label(egui::RichText::new(pname).font(semibold(11.0)).color(t.text_secondary));
                    }
                    let current = Some(*c) == set || *c == *component;
                    if menu_item(ui, &format!("{}{name}", if current { "✓ " } else { "" }), "") && !current {
                        app.run("instance.swap", json!({"ids": [id], "component": c.0}));
                        ui.close();
                    }
                }
            });
        });
        if missing {
            if icons::button(ui, Icon::Component, 24.0, false, tr("Restore main component")).clicked() {
                app.run("component.restoreMain", json!({"id": id}));
            }
        } else if icons::button(ui, Icon::Component, 24.0, false, tr("Go to main component")).clicked() {
            app.run("component.goToMain", json!({"id": id}));
        }
    });
    // The main component's description (the set's when the variant has none).
    let desc = [Some(*component), set]
        .into_iter()
        .flatten()
        .filter_map(|c| match doc.node(c).map(|n| &n.kind) {
            Some(NodeKind::Component { description, .. } | NodeKind::ComponentSet { description }) => Some(description.trim().to_string()),
            _ => None,
        })
        .find(|d| !d.is_empty());
    if let Some(d) = desc {
        ui.horizontal(|ui| {
            ui.add_space(crate::widgets::PAD);
            ui.add(egui::Label::new(egui::RichText::new(d).font(crate::theme::regular(11.0)).color(t.text_secondary)).wrap());
        });
        ui.add_space(4.0);
    }
    // A nested instance inside a main component can expose its properties on the main's instances.
    if nested_in_main(doc, id) {
        row(ui, 32.0, |ui| {
            if widgets::checkbox(ui, n.exposed, tr("Expose properties to instances")) {
                app.run("component.exposeInstance", json!({"id": id, "exposed": !n.exposed}));
            }
        });
    }
    instance_props(app, ui, doc, n, &comps);
    // Exposed nested instances: their properties, under their names.
    if !nested {
        for c in lizard_engine::components::exposed_copies(doc, id) {
            let Some(cn) = doc.node(c) else { continue };
            row(ui, 28.0, |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Instance, crate::theme::COMPONENT);
                ui.label(egui::RichText::new(&cn.name).font(semibold(11.0)).color(t.text));
            });
            instance_props(app, ui, doc, cn, &comps);
        }
    }
}

/// An instance placed inside a main component (not inside another instance).
fn nested_in_main(doc: &Document, id: NodeId) -> bool {
    !inside_instance(doc, id) && lizard_engine::components::owning_component(doc, id).is_some_and(|c| c != id)
}

/// Variant and component property controls for instance `n`.
fn instance_props(app: &mut LizardApp, ui: &mut Ui, doc: &Document, n: &Node, comps: &[(NodeId, String)]) {
    let NodeKind::Instance { component, props } = &n.kind else { return };
    let id = n.id;
    let set = variant_set(doc, *component);
    let slots = slots_of(doc, id);
    // Variant properties.
    if let Some(set) = set {
        let current = doc.node(*component).map(|c| variant_props(&c.name)).unwrap_or_default();
        for (k, vals) in variant_options(doc, set) {
            let cur = current.get(&k).cloned().unwrap_or_default();
            row(ui, 32.0, |ui| {
                prop_label(ui, &k, PropertyControllerKind::Variant);
                let w = ui.available_width().max(60.0);
                // A string variable naming one of the values can choose it.
                crate::variable_ui::bindable(app, ui, &[id], &format!("{PROP_BINDING}{k}"), VariableType::String, w, |app, ui| {
                    let resp = crate::primitives::Select::trigger(ui, &cur, w, None);
                    menu_popup(&resp).show(|ui| {
                        for v in &vals {
                            if menu_item(ui, v, "") {
                                app.run("instance.setProperties", json!({"ids": [id], "properties": {k.clone(): v}}));
                                ui.close();
                            }
                        }
                    });
                });
            });
        }
    }
    // Boolean, text and instance-swap properties.
    for (k, d) in prop_defs(doc, *component) {
        let v = props.get(&k).cloned().unwrap_or_else(|| lizard_engine::components::prop_default_value(&d));
        row(ui, 32.0, |ui| {
            prop_label(ui, &k, type_kind(&d));
            let w = ui.available_width().max(60.0);
            // Boolean and text values can come from a boolean or string variable.
            let bind_field = format!("{PROP_BINDING}{k}");
            match d {
                PropDef::Boolean { .. } => {
                    crate::variable_ui::bindable(app, ui, &[id], &bind_field, VariableType::Boolean, w, |app, ui| {
                        let on = v.as_bool().unwrap_or(false);
                        if widgets::checkbox(ui, on, "") {
                            app.run("instance.setProperties", json!({"ids": [id], "properties": {k.clone(): !on}}));
                        }
                    });
                }
                PropDef::Text { .. } => {
                    crate::variable_ui::bindable(app, ui, &[id], &bind_field, VariableType::String, w, |app, ui| {
                        let cur = v.as_str().unwrap_or_default().to_string();
                        if let Some(ns) = widgets::text_field(ui, Id::new(("iprop", id, k.as_str())), &cur, w, "")
                            && ns != cur
                        {
                            app.run("instance.setProperties", json!({"ids": [id], "properties": {k.clone(): ns}}));
                        }
                    });
                }
                PropDef::InstanceSwap { preferred, .. } => {
                    let cur = lizard_engine::params::id_of(&v);
                    let label = cur.and_then(|c| doc.node(c)).map(|c| c.name.clone()).unwrap_or_else(|| tr("None").into());
                    let resp = crate::primitives::Select::trigger(ui, &label, w, Some(Icon::Instance));
                    menu_popup(&resp).width(200.0).show(|ui| {
                        let t = Tokens::get(ui.ctx());
                        let heading = |ui: &mut Ui, text: &str| ui.label(egui::RichText::new(text).font(semibold(11.0)).color(t.text_secondary));
                        let mut pick = |ui: &mut Ui, c: NodeId, name: &str| {
                            let current = cur == Some(c);
                            if menu_item(ui, &format!("{}{name}", if current { "✓ " } else { "" }), "") {
                                if !current {
                                    app.run("instance.setProperties", json!({"ids": [id], "properties": {k.clone(): c.0}}));
                                }
                                ui.close();
                            }
                        };
                        let preferred: Vec<(NodeId, String)> = preferred.iter().filter_map(|c| doc.node(*c).map(|n| (*c, n.name.clone()))).collect();
                        if !preferred.is_empty() {
                            heading(ui, tr("Preferred"));
                            for (c, name) in &preferred {
                                pick(ui, *c, name);
                            }
                            heading(ui, tr("All components"));
                        }
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            for (c, name) in comps {
                                pick(ui, *c, name);
                            }
                        });
                    });
                }
                PropDef::Variant { .. } => {}
                PropDef::Slot { preferred } => {
                    if let Some((slot, _)) = slots.iter().find(|(_, name)| *name == k) {
                        slot_fill(app, ui, doc, *slot, &preferred, comps, w);
                    }
                }
            }
        });
    }
}

fn prop_label(ui: &mut Ui, name: &str, icon: PropertyControllerKind) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(84.0, 20.0), Sense::hover());
    icon.paint(ui.painter(), egui::Rect::from_min_size(egui::pos2(r.min.x, r.center().y - 6.0), vec2(12.0, 12.0)), t.icon_secondary);
    let g = ui.painter().layout(name.to_string(), crate::theme::regular(11.0), t.text_secondary, r.width() - 18.0);
    ui.painter().galley(egui::pos2(r.min.x + 18.0, r.center().y - g.size().y / 2.0), g, t.text_secondary);
}
