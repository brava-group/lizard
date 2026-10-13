//! Widgets: layers on the canvas that a plugin draws (plugin API 3). A widget is a frame owned by
//! its plugin: the plugin describes it ([`WidgetRender`]: frames with auto layout, text, shapes and
//! images) and FigCraft builds the layers inside it, locked, so the widget moves as one layer.
//! Each widget keeps its plugin's id and a small state (a JSON object) in its own layer data, so it
//! is saved, copied, undone, exported and synced in a live session like any layer. Clicking a part
//! of it the plugin named as a target calls the plugin back with that state, in a fresh sandbox
//! (deterministic: the state is all it remembers), as one undo step; the plugin hands back the new
//! state and layers. A callback can only rebuild its own widget; other layers change only through
//! the commands its manifest permits. See `docs/plugins.md`.
//!
//! Plugin API 4 adds more events, each a callback like a click: a text the user edited in place
//! ([`Input`], `widget.setText`), an item of the widget's property menu (`widget.menuAction`) and a
//! timer the widget asked for (`widget.fireTimer`; [`run_timers`] in the app). Hover styles and
//! tooltips are FigCraft's to show ([`set_hover`]): the plugin isn't called for pointer moves.
//! Everything read back from layer data is checked again: a file or a peer may hold anything.
//!
//! **Timers** are kept with the open document, not in it: closing the file cancels them, and they
//! run only while the document is in front. When a callback asks for one, the widget's layer gets
//! a stamp only this session knows, and the timer fires only while that stamp is still on the
//! layer. So in a live session a widget's timer runs for whoever started it last and for nobody
//! else (a later click by someone else replaces the stamp, which stops the earlier timer), two
//! people's timers never take turns rewriting a widget, and undoing past the callback that
//! started a timer stops it. A file you may only view runs no timers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use figcraft_color::{Color, ColorStop, Gradient, GradientKind, ImageScaleMode, Paint};
use figcraft_doc::{Document, Effect, EffectKind, Node, NodeId, NodeKind, Shadow, TextData, VectorNetwork};
use figcraft_geom::{Affine, Point, Shape};
use figcraft_plugin::widget::{GradientShape, MAX_IMAGE_BYTES, MAX_SIZE, MAX_STATE_BYTES, MAX_TEXT, WidgetHover, check_menu};
use figcraft_plugin::{Instance, Manifest, RunOutput, WidgetFit, WidgetKind, WidgetNode, WidgetRender};
pub use figcraft_plugin::{MenuItem, MenuKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use web_time::{Duration, Instant};

use crate::cmd::CommandSpec;
use crate::params::{get, id, num, string};
use crate::plugins::{InstalledPlugin, call_plugin, installed, limits, module_of, plugin_error};
use crate::{EngineError, Result, Session, bad, spec};

/// Layer data FigCraft keeps for widgets starts with this; plugins can't write it.
pub const DATA_PREFIX: &str = "figcraft.widget";
/// On a widget's own layer: its plugin's id, its state (JSON), its property menu (JSON) and the
/// stamp of the session whose timer for it is running.
const PLUGIN: &str = "figcraft.widget";
const STATE: &str = "figcraft.widget.state";
const MENU: &str = "figcraft.widget.menu";
const TIMER: &str = "figcraft.widget.timer";
/// On a layer inside a widget: the name its plugin gets when it's clicked, the editable text it
/// is ([`Input`] as JSON), its hover style (JSON) and its tooltip.
const TARGET: &str = "figcraft.widget.target";
const INPUT: &str = "figcraft.widget.input";
const HOVER: &str = "figcraft.widget.hover";
const TOOLTIP: &str = "figcraft.widget.tooltip";
/// Images in a widget are at most this many pixels on a side.
const MAX_IMAGE_SIDE: u32 = 4096;
/// The stored menu and hover style are read back only up to this size.
const MAX_DATA_BYTES: usize = 64 * 1024;
/// While the document is busy (a drag, text being typed) a due timer waits this long.
const TIMER_RETRY: Duration = Duration::from_millis(250);

/// A text in a widget the user can edit in place: its name for the plugin, the most characters
/// it takes and whether it may have several lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub max: usize,
    #[serde(default)]
    pub multiline: bool,
}

impl Input {
    /// `text` as this input takes it: no control characters, one line unless it's multiline, and
    /// no longer than its maximum.
    pub fn clean(&self, text: &str) -> String {
        let line_break = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
        text.replace("\r\n", "\n")
            .chars()
            .map(|c| match c {
                c if line_break(c) && self.multiline => '\n',
                c if line_break(c) || c == '\t' => ' ',
                c => c,
            })
            .filter(|c| !c.is_control() || *c == '\n')
            .take(self.max.min(MAX_TEXT))
            .collect()
    }
}

/// A widget's pending timer.
#[derive(Clone, Debug)]
pub struct Timer {
    widget: NodeId,
    due: Instant,
    /// What [`TIMER`] on the widget's layer must still say when it fires.
    stamp: String,
}

/// What the pointer is over in a widget.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// A clickable part: its target.
    Target(String),
    /// An editable text: its layer.
    Input(NodeId),
}

/// Whether `n` is a widget's own layer.
pub fn is_widget(n: &Node) -> bool {
    matches!(n.kind, NodeKind::Frame) && n.plugin_data.contains_key(PLUGIN)
}

/// The widget layer `id` is, or is inside (the innermost).
pub fn widget_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    std::iter::once(id).chain(doc.ancestors(id)).find(|a| doc.node(*a).is_some_and(is_widget))
}

/// The visible layers of the widget `hit` is (or is in) whose box holds page point `p`, back to
/// front, and the widget. The layers inside a widget are locked, so a hit stops at the widget.
fn layers_at(doc: &Document, hit: NodeId, p: Point) -> Option<(NodeId, Vec<&Node>)> {
    let w = widget_of(doc, hit)?;
    let at =
        doc.subtree(w).into_iter().filter(|i| doc.world_bounds(*i).is_some_and(|b| b.contains(p))).filter_map(|i| doc.node(i).filter(|n| n.visible));
    Some((w, at.collect()))
}

/// The part of a widget at page point `p` that takes a click, given the layer a click there hits:
/// the widget, and the front-most clickable part or editable text whose box holds the point.
pub fn part_at(doc: &Document, hit: NodeId, p: Point) -> Option<(NodeId, Part)> {
    let (w, layers) = layers_at(doc, hit, p)?;
    let part = layers.iter().rev().find_map(|n| match n.plugin_data.get(TARGET) {
        Some(t) => Some(Part::Target(t.clone())),
        None => input_of(doc, n.id).map(|_| Part::Input(n.id)),
    })?;
    Some((w, part))
}

/// The clickable part of a widget at page point `p`: the widget and its target.
pub fn target_at(doc: &Document, hit: NodeId, p: Point) -> Option<(NodeId, String)> {
    match part_at(doc, hit, p)? {
        (w, Part::Target(t)) => Some((w, t)),
        _ => None,
    }
}

/// The front-most layer with a hover style at page point `p` of the widget `hit` is (or is in).
pub fn hover_at(doc: &Document, hit: NodeId, p: Point) -> Option<NodeId> {
    layers_at(doc, hit, p)?.1.iter().rev().find(|n| n.plugin_data.contains_key(HOVER)).map(|n| n.id)
}

/// The tooltip of the front-most layer that has one at page point `p` of the widget `hit` is in.
pub fn tooltip_at(doc: &Document, hit: NodeId, p: Point) -> Option<String> {
    let tip = layers_at(doc, hit, p)?.1.iter().rev().find_map(|n| n.plugin_data.get(TOOLTIP))?;
    Some(tip.chars().filter(|c| !c.is_control()).take(200).collect::<String>()).filter(|t| !t.trim().is_empty())
}

/// The editable text layer `id` is, if it is one (in a widget).
pub fn input_of(doc: &Document, id: NodeId) -> Option<Input> {
    let n = doc.node(id).filter(|n| matches!(n.kind, NodeKind::Text(_)))?;
    let mut input: Input = serde_json::from_str(n.plugin_data.get(INPUT).filter(|s| s.len() <= 1024)?).ok()?;
    let named =
        !input.name.is_empty() && input.name.len() <= 64 && input.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    input.max = input.max.clamp(1, MAX_TEXT);
    (named && widget_of(doc, id).is_some_and(|w| w != id)).then_some(input)
}

/// `doc` with layer `part` in its hover style, if it has one.
pub fn hovered(doc: &Document, part: NodeId) -> Option<Document> {
    let style = doc.node(part)?.plugin_data.get(HOVER).filter(|s| s.len() <= MAX_DATA_BYTES)?;
    let h: WidgetHover = serde_json::from_str(style).ok()?;
    let colour = |c: &Option<String>| c.as_deref().filter(|c| c.len() <= 9).and_then(Color::from_hex).map(Paint::solid);
    let mut look = doc.clone();
    let n = look.get_mut(part).ok()?;
    if let Some(fill) = colour(&h.fill) {
        n.fills = vec![fill];
    }
    if let Some(stroke) = colour(&h.stroke) {
        n.strokes = vec![stroke];
        n.stroke.weight = n.stroke.weight.max(1.0);
    }
    if let Some(o) = h.opacity.filter(|o| o.is_finite()) {
        n.opacity = o.clamp(0.0, 1.0) as f32;
    }
    Some(look)
}

/// Show the widget part `part` (with the page point the pointer is at) in its hover style, or no
/// part. The look is a live preview: the document itself, its undo history and what a live
/// session syncs stay as they are, and any command ends it.
pub fn set_hover(s: &mut Session, part: Option<(NodeId, Point)>) {
    let Ok(ds) = s.ds_mut() else { return };
    if ds.widget_hover.map(|h| h.0) == part.map(|h| h.0) {
        ds.widget_hover = part;
        return;
    }
    ds.end_widget_hover();
    let Some((id, _)) = part else { return };
    if ds.interaction.is_some() || ds.text_edit.is_some() || ds.preview.is_some() || ds.text_preview.is_some() {
        return;
    }
    let Some(look) = hovered(&ds.doc, id) else { return };
    ds.text_preview = Some(std::mem::replace(&mut ds.doc, Arc::new(look)));
    ds.widget_hover = part;
    ds.revision += 1;
}

/// Put the hover look back after a change that wasn't the pointer's (a timer's callback, an edit
/// from a live session): on what is now under page point `at`, where the pointer still is.
pub(crate) fn rehover(s: &mut Session, at: Option<Point>) {
    let (Some(p), Ok(ds)) = (at, s.ds()) else { return };
    let (doc, page) = (ds.doc.clone(), ds.page);
    let part = s.hit(&doc, page, p, 0.0).and_then(|hit| hover_at(&doc, hit, p));
    set_hover(s, part.map(|part| (part, p)));
}

/// A widget's state (an empty object when it has none or it's unusable).
fn state_of(doc: &Document, w: NodeId) -> Map<String, Value> {
    doc.node(w)
        .and_then(|n| n.plugin_data.get(STATE))
        .filter(|s| s.len() <= MAX_STATE_BYTES)
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// The targets inside widget `w`, each once, in layer order.
fn targets(doc: &Document, w: NodeId) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in doc.subtree(w).into_iter().filter_map(|i| doc.node(i)?.plugin_data.get(TARGET).cloned()) {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// The editable texts inside widget `w` (each name once), in layer order.
fn inputs(doc: &Document, w: NodeId) -> Vec<(NodeId, Input)> {
    let mut out: Vec<(NodeId, Input)> = Vec::new();
    for (i, input) in doc.subtree(w).into_iter().filter_map(|i| Some((i, input_of(doc, i)?))) {
        if !out.iter().any(|(_, o)| o.name == input.name) {
            out.push((i, input));
        }
    }
    out
}

/// Widget `w`'s property menu (empty when it has none or it's unusable): what a selected widget
/// shows near it and in the Design panel.
pub fn menu_of(doc: &Document, w: NodeId) -> Vec<MenuItem> {
    let stored = doc.node(w).filter(|n| is_widget(n)).and_then(|n| n.plugin_data.get(MENU)).filter(|s| s.len() <= MAX_DATA_BYTES);
    let mut menu: Vec<MenuItem> = stored.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let fine = check_menu(&mut menu).is_ok() && menu.iter().all(|i| i.kind != MenuKind::Link || crate::dev_resources::check_url(&i.url).is_ok());
    if fine { menu } else { Vec::new() }
}

/// A stamp no other session makes: this process's start in nanoseconds and a count.
fn stamp() -> String {
    static START: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    static COUNT: AtomicU64 = AtomicU64::new(1);
    let start = START.get_or_init(|| web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!("{start:x}-{}", COUNT.fetch_add(1, Ordering::Relaxed))
}

/// Widget `w`'s pending timer in the active document: one this session started that nobody
/// replaced since.
fn timer_of(s: &Session, w: NodeId) -> Option<&Timer> {
    let ds = s.ds().ok()?;
    let on_layer = ds.doc.node(w).filter(|n| is_widget(n))?.plugin_data.get(TIMER)?;
    ds.widget_timers.iter().find(|t| t.widget == w && t.stamp == *on_layer)
}

/// Start (or with 0 milliseconds, stop) widget `w`'s timer: at most one is pending.
fn arm(s: &mut Session, w: NodeId, ms: u32) -> Result<()> {
    let ds = s.ds_mut()?;
    ds.widget_timers.retain(|t| t.widget != w);
    if ms > 0 {
        let stamp = stamp();
        ds.doc_mut().get_mut(w)?.plugin_data.insert(TIMER.into(), stamp.clone());
        ds.widget_timers.push(Timer { widget: w, due: Instant::now() + Duration::from_millis(u64::from(ms)), stamp });
    }
    Ok(())
}

/// How long until the active document's next widget timer is due, in seconds (`None`: none is
/// pending). The app sleeps until then; nothing else wakes it for timers.
pub fn next_timer(s: &Session) -> Option<f64> {
    let ds = s.ds().ok()?;
    let now = Instant::now();
    ds.widget_timers.iter().map(|t| t.due.saturating_duration_since(now).as_secs_f64()).reduce(f64::min)
}

/// Fire the active document's widget timers that are due: each calls its widget's plugin back
/// (`widget.fireTimer`). Timers whose widget is gone, was drawn again by someone else or undone
/// past are dropped; while the document is busy (a drag, text being typed) they wait; a document
/// you can't edit runs none. Returns what went wrong, to show.
pub fn run_timers(s: &mut Session) -> Vec<String> {
    let now = Instant::now();
    let Ok(ds) = s.ds() else { return Vec::new() };
    if !ds.widget_timers.iter().any(|t| t.due <= now) {
        return Vec::new();
    }
    if s.active_read_only().is_some() || ds.preview.is_some() {
        if let Ok(ds) = s.ds_mut() {
            ds.widget_timers.clear();
        }
        return Vec::new();
    }
    // The hover look steps aside for the callback and comes back where the pointer still is.
    let Ok(ds) = s.ds_mut() else { return Vec::new() };
    let hover = ds.end_widget_hover();
    if ds.interaction.is_some() || ds.text_edit.is_some() || ds.text_preview.is_some() {
        ds.widget_timers.iter_mut().filter(|t| t.due <= now).for_each(|t| t.due = now + TIMER_RETRY);
        return Vec::new();
    }
    let due: Vec<NodeId> = ds.widget_timers.iter().filter(|t| t.due <= now).map(|t| t.widget).collect();
    let mut errors = Vec::new();
    for w in due {
        if timer_of(s, w).is_none() {
            if let Ok(ds) = s.ds_mut() {
                ds.widget_timers.retain(|t| t.widget != w);
            }
        } else if let Err(e) = s.execute("widget.fireTimer", &json!({"id": w})) {
            errors.push(e.to_string());
        }
    }
    rehover(s, hover);
    errors
}

/// Installed widget plugin `plugin`, its manifest checked again (the preferences may have been
/// edited by hand).
fn widget_plugin(s: &Session, cmd: &str, plugin: &str) -> Result<InstalledPlugin> {
    let mut p = installed(s, cmd, plugin)?.clone();
    p.manifest = Manifest::from_value(&json!(p.manifest)).map_err(|e| bad(cmd, e))?;
    if !p.manifest.widget {
        return Err(bad(cmd, format!("{} doesn't draw a widget", p.manifest.name)));
    }
    Ok(p)
}

/// Call widget `w`'s plugin `p` back with `event` (what happened to it: a click's `target`, an
/// edited text, a menu item, a timer; null when it's inserted) and build what it hands back.
/// Part of the caller's undo step; a failure rolls the whole step back.
fn callback(s: &mut Session, p: &InstalledPlugin, w: NodeId, event: &Value, fuel: Option<f64>) -> Result<RunOutput> {
    let m = &p.manifest;
    // A file you may only view calls no plugin: nothing it drew could be kept.
    if let Some(why) = s.active_read_only() {
        return Err(EngineError::Disabled(format!("{}'s widget", m.name), why.into()));
    }
    let wasm = module_of(s, p)?;
    let state = state_of(s.doc()?, w);
    let input =
        json!({"api": m.api, "command": "widget", "params": {}, "selection": s.selection(), "widget": {"id": w, "state": state, "event": event}});
    let mut instance = Instance::new(&wasm, &limits(fuel, m)).map_err(|e| plugin_error(&m.name, e))?;
    let mut out = call_plugin(s, m, &m.name, &mut instance, &input.to_string()).map_err(|(e, _)| e)?;
    match out.widget.take() {
        Some(r) => build(s, &m.id, w, &r)?,
        None if event.is_null() => return Err(bad("widget.insert", format!("{} drew no widget", m.name))),
        None => {}
    }
    if let Some(ms) = out.timer {
        arm(s, w, ms)?;
    }
    Ok(out)
}

/// Rebuild widget `w` of `plugin` from `r`: its own look and state, and new locked layers inside.
fn build(s: &mut Session, plugin: &str, w: NodeId, r: &WidgetRender) -> Result<()> {
    let cmd = "widget.render";
    let ds = s.ds_mut()?;
    let before = ds.doc.clone();
    let doc = ds.doc_mut();
    let n = doc.get(w).map_err(|_| bad(cmd, "the widget was removed while its plugin ran"))?;
    if !is_widget(n) || n.plugin_data.get(PLUGIN).map(String::as_str) != Some(plugin) {
        return Err(bad(cmd, "the layer is no longer this plugin's widget"));
    }
    for c in n.children.clone() {
        doc.remove(c)?;
    }
    crate::cmd::props::apply(cmd, doc, w, &props(&r.root, true))?;
    dress(cmd, doc, w, &r.root)?;
    let n = doc.get_mut(w)?;
    n.locked = false;
    if !r.root.name.is_empty() {
        n.name = r.root.name.clone();
    }
    if let Some(state) = &r.state {
        n.plugin_data.insert(STATE.into(), Value::Object(state.clone()).to_string());
    }
    if let Some(menu) = &r.menu {
        for link in menu.iter().filter(|i| i.kind == MenuKind::Link) {
            crate::dev_resources::check_url(&link.url).map_err(|e| bad(cmd, format!("menu item `{}`: {e}", link.name)))?;
        }
        n.plugin_data.insert(MENU.into(), json!(menu).to_string());
    }
    for c in &r.root.children {
        add(doc, w, c, cmd)?;
    }
    crate::normalize(doc, &before);
    Ok(())
}

/// Add layer `d` (and what it holds) inside `parent`, locked.
fn add(doc: &mut Document, parent: NodeId, d: &WidgetNode, cmd: &str) -> Result<()> {
    let (mut width, mut height) = (d.width.unwrap_or(100.0), d.height.unwrap_or(100.0));
    let (kind, name) = match d.kind {
        WidgetKind::Frame => (NodeKind::Frame, "Frame"),
        WidgetKind::Text => (NodeKind::Text(TextData { characters: d.text.clone(), ..TextData::default() }), "Text"),
        WidgetKind::Rectangle => (NodeKind::Rectangle, "Rectangle"),
        WidgetKind::Ellipse => (NodeKind::Ellipse { start: 0.0, sweep: std::f64::consts::TAU, inner: 0.0 }, "Ellipse"),
        WidgetKind::Image => (NodeKind::Rectangle, "Image"),
        WidgetKind::Line => {
            height = 0.0;
            (NodeKind::Line, "Line")
        }
        WidgetKind::Vector => {
            let mut path = crate::cmd::create::parse_path_data(&d.path).ok_or_else(|| bad(cmd, "a vector's `path` isn't valid SVG path data"))?;
            let bb = path.bounding_box();
            let finite = [bb.x0, bb.y0, bb.x1, bb.y1].iter().all(|v| v.is_finite() && v.abs() <= MAX_SIZE);
            if !finite || path.elements().is_empty() {
                return Err(bad(cmd, format!("a vector's `path` must stay within ±{MAX_SIZE}")));
            }
            // In its own box, scaled to the size asked for.
            let fit = |want: Option<f64>, have: f64| want.filter(|_| have > 1e-9).map_or(1.0, |w| w / have);
            path.apply_affine(Affine::scale_non_uniform(fit(d.width, bb.width()), fit(d.height, bb.height())) * Affine::translate((-bb.x0, -bb.y0)));
            (width, height) = (d.width.unwrap_or(bb.width()), d.height.unwrap_or(bb.height()));
            (NodeKind::Vector { network: VectorNetwork::from_path(&path) }, "Vector")
        }
    };
    // Cut after `maxLines`: a fixed box that many lines high unless a height is given.
    if let (WidgetKind::Text, Some(lines), None) = (d.kind, d.max_lines, d.height) {
        height = (lines * d.size.unwrap_or(14.0) * 1.3).ceil();
    }
    let name = if d.name.is_empty() { name.to_string() } else { d.name.clone() };
    let id = doc.alloc_id();
    let mut n = Node::new(id, kind, name, width, height);
    n.transform = Affine::translate((d.x.unwrap_or(0.0), d.y.unwrap_or(0.0)));
    n.locked = true;
    if d.kind == WidgetKind::Image {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(d.data.trim()).map_err(|e| bad(cmd, format!("an image's `data`: {e}")))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(bad(cmd, format!("an image is larger than {} KB", MAX_IMAGE_BYTES / 1024)));
        }
        let (hash, iw, ih) = figcraft_format::add_image(doc, bytes).map_err(|e| bad(cmd, e.to_string()))?;
        if iw > MAX_IMAGE_SIDE || ih > MAX_IMAGE_SIDE {
            return Err(bad(cmd, format!("an image is larger than {MAX_IMAGE_SIDE} pixels on a side")));
        }
        n.width = d.width.unwrap_or(f64::from(iw));
        n.height = d.height.unwrap_or(f64::from(ih));
        let mode = match d.fit {
            None | Some(WidgetFit::Fill) => ImageScaleMode::Fill,
            Some(WidgetFit::Fit) => ImageScaleMode::Fit,
            Some(WidgetFit::Crop) => ImageScaleMode::Crop,
            Some(WidgetFit::Tile) => ImageScaleMode::Tile,
        };
        n.fills = vec![Paint::image(hash, mode)];
    }
    doc.insert(n, parent, None)?;
    crate::cmd::props::apply(cmd, doc, id, &props(d, false))?;
    dress(cmd, doc, id, d)?;
    d.children.iter().try_for_each(|c| add(doc, id, c, cmd))
}

/// What `node.set` has no plain property for, on built layer `id` (the widget itself too): its
/// gradient and shadow, and the layer data that makes it a part (target, input, hover, tooltip).
fn dress(cmd: &str, doc: &mut Document, id: NodeId, d: &WidgetNode) -> Result<()> {
    let colour = |c: &str| Color::from_hex(c).ok_or_else(|| bad(cmd, "a colour isn't hex"));
    let n = doc.get_mut(id)?;
    if let Some(g) = &d.gradient {
        let stops = g.stops.iter().map(|s| Ok(ColorStop { position: s.at as f32, color: colour(&s.color)? })).collect::<Result<Vec<_>>>()?;
        let mut gradient = match g.kind {
            GradientShape::Linear => {
                let (sin, cos) = g.angle.unwrap_or(90.0).to_radians().sin_cos();
                let mut l = Gradient::default_for(GradientKind::Linear, Color::BLACK);
                (l.from, l.to) = ((0.5 - cos / 2.0, 0.5 - sin / 2.0), (0.5 + cos / 2.0, 0.5 + sin / 2.0));
                l.square_linear_width(n.width, n.height);
                l
            }
            GradientShape::Radial => Gradient::default_for(GradientKind::Radial, Color::BLACK),
        };
        gradient.stops = stops;
        n.fills = vec![Paint::gradient(gradient)];
    }
    n.effects = match &d.shadow {
        Some(s) => {
            let color = s.color.as_deref().map(colour).transpose()?.unwrap_or(Shadow::default().color);
            let shadow = Shadow { color, offset: (s.x, s.y), radius: s.blur, spread: s.spread, ..Shadow::default() };
            vec![Effect { kind: EffectKind::DropShadow(shadow), visible: true }]
        }
        None => Vec::new(),
    };
    let input = (!d.input.is_empty())
        .then(|| json!(Input { name: d.input.clone(), max: d.max_length.map_or(MAX_TEXT, |m| m as usize), multiline: d.multiline }).to_string());
    let data = [
        (TARGET, (!d.target.is_empty()).then(|| d.target.clone())),
        (INPUT, input),
        (HOVER, d.hover.as_ref().map(|h| json!(h).to_string())),
        (TOOLTIP, (!d.tooltip.is_empty()).then(|| d.tooltip.clone())),
    ];
    for (key, value) in data {
        match value {
            Some(v) => n.plugin_data.insert(key.into(), v),
            None => n.plugin_data.remove(key),
        };
    }
    Ok(())
}

/// `node.set` properties for described layer `d` (validated already); `root`: the widget itself,
/// which keeps its place and size unless the description gives one.
fn props(d: &WidgetNode, root: bool) -> Map<String, Value> {
    let mut p = Map::new();
    let mut set = |k: &str, v: Value| {
        p.insert(k.into(), v);
    };
    match d.kind {
        WidgetKind::Text => {
            if let Some(c) = &d.color {
                set("fills", json!([c]));
            }
            set("fontSize", json!(d.size.unwrap_or(14.0)));
            match d.weight {
                Some(w) => set("fontWeight", json!(w)),
                None if d.bold => set("fontStyle", json!("Bold")),
                None => {}
            }
            if let Some(a) = d.text_align {
                set("textAlign", json!(a));
            }
            // A text with a width wraps (and is cut after `maxLines`); one without grows with its text.
            let resize = match (d.width, d.max_lines) {
                (Some(_), Some(lines)) => {
                    set("maxLines", json!(lines));
                    "truncate"
                }
                (Some(_), None) => "height",
                (None, _) => "widthAndHeight",
            };
            set("textAutoResize", json!(resize));
        }
        WidgetKind::Image => {}
        _ => set("fills", json!(d.fill.iter().collect::<Vec<_>>())),
    }
    if let Some(c) = d.stroke.as_deref().or((d.kind == WidgetKind::Line).then_some("#000000")) {
        set("strokes", json!([c]));
        set("strokeWeight", json!(d.stroke_weight.unwrap_or(1.0)));
    } else if root {
        set("strokes", json!([]));
    }
    if matches!(d.kind, WidgetKind::Frame | WidgetKind::Rectangle | WidgetKind::Image) {
        set("cornerRadius", json!(d.radius.map_or([0.0; 4], figcraft_plugin::Sides::each)));
    }
    set("opacity", json!(d.opacity.unwrap_or(1.0)));
    if d.kind == WidgetKind::Frame {
        match d.layout {
            Some(l) => {
                set("layout", json!(l));
                set("gap", json!(d.gap.unwrap_or(0.0)));
                set("padding", json!(d.padding.map_or([0.0; 4], figcraft_plugin::Sides::each)));
                set("crossAlign", json!(d.align.map_or(json!("center"), |a| json!(a))));
                set("mainAlign", json!(d.justify.map_or(json!("min"), |a| json!(a))));
                set("sizingH", json!(if d.width.is_some() { "fixed" } else { "hug" }));
                set("sizingV", json!(if d.height.is_some() { "fixed" } else { "hug" }));
            }
            None => set("layout", Value::Null),
        }
    }
    // What fills is in an auto layout frame with room on that axis (the description was checked).
    for (k, fill) in [("sizingH", d.fill_width), ("sizingV", d.fill_height)] {
        if fill {
            set(k, json!("fill"));
        }
    }
    // A new layer got its size when it was made.
    if root {
        for (k, v) in [("width", d.width), ("height", d.height)] {
            if let Some(v) = v {
                set(k, json!(v));
            }
        }
    }
    p
}

/// A widget as `widget.list` describes it, without where it is: its state, what can be done to
/// it (targets to click, texts to edit, its property menu) and its pending timer.
fn describe(s: &Session, w: NodeId) -> Result<Value> {
    let doc = s.doc()?;
    let texts: Vec<Value> = inputs(doc, w)
        .into_iter()
        .map(|(i, input)| {
            let text = doc.node(i).and_then(|n| n.kind.text()).map(|t| t.characters.clone()).unwrap_or_default();
            json!({"name": input.name, "text": text, "maxLength": input.max, "multiline": input.multiline})
        })
        .collect();
    let timer = timer_of(s, w).map(|t| t.due.saturating_duration_since(Instant::now()).as_millis() as u64);
    Ok(
        json!({"id": w, "plugin": doc.get(w)?.plugin_data.get(PLUGIN), "state": state_of(doc, w), "targets": targets(doc, w), "inputs": texts, "menu": menu_of(doc, w), "timer": timer}),
    )
}

/// What the commands that call a widget's plugin back return.
fn report(s: &Session, w: NodeId, out: RunOutput) -> Result<Value> {
    let mut v = describe(s, w)?;
    v["output"] = serde_json::from_str::<Value>(&out.output).unwrap_or(Value::String(out.output));
    (v["logs"], v["calls"], v["fuel"]) = (json!(out.logs), json!(out.calls), json!(out.fuel));
    Ok(v)
}

fn insert(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "widget.insert";
    let fuel = num(p, "fuel");
    let plugin = string(p, "plugin").ok_or_else(|| bad(cmd, "`plugin` required (a widget plugin's id; see plugin.list)"))?;
    let wp = widget_plugin(s, cmd, &plugin)?;
    let m = &wp.manifest;
    let (x, y) = (num(p, "x").filter(|v| v.abs() < 1e7), num(p, "y").filter(|v| v.abs() < 1e7));
    let page = s.page()?;
    let (cx, cy) = {
        let v = &s.ds()?.view;
        (v.cx, v.cy)
    };
    let (w, out) = s.grouped(&m.name, |s| {
        let doc = s.ds_mut()?.doc_mut();
        let w = doc.alloc_id();
        let mut n = Node::new(w, NodeKind::Frame, m.name.clone(), 100.0, 100.0);
        n.transform = Affine::translate((x.unwrap_or(cx), y.unwrap_or(cy)));
        n.fills.clear();
        n.plugin_data.insert(PLUGIN.into(), m.id.clone());
        doc.insert(n, page, None)?;
        let out = callback(s, &wp, w, &Value::Null, fuel)?;
        let ds = s.ds_mut()?;
        // Without a position, centred in the view on whole pixels.
        if x.is_none() && y.is_none() {
            let (ww, wh) = ds.doc.node(w).map_or((0.0, 0.0), |n| (n.width, n.height));
            ds.doc_mut().get_mut(w)?.transform = Affine::translate(((cx - ww / 2.0).round(), (cy - wh / 2.0).round()));
        }
        ds.selection = vec![w];
        Ok((w, out))
    })?;
    report(s, w, out)
}

/// The widget command `cmd` is about: the one its `id` is (or is in), else the one selected.
fn widget_id(s: &Session, cmd: &str, p: &Value) -> Result<NodeId> {
    let doc = s.doc()?;
    let nid = match (id(p, "id"), s.selection().as_slice()) {
        (Some(nid), _) => nid,
        (None, [only]) => *only,
        (None, _) => return Err(bad(cmd, "`id` required (a widget, or a layer in one; without it, the one selected layer)")),
    };
    widget_of(doc, nid).ok_or_else(|| bad(cmd, format!("{nid} isn't a widget or inside one")))
}

/// The widget command `cmd` is about (see [`widget_id`]), and its plugin.
fn widget_for(s: &Session, cmd: &str, p: &Value) -> Result<(NodeId, InstalledPlugin)> {
    let w = widget_id(s, cmd, p)?;
    let plugin = s.doc()?.node(w).and_then(|n| n.plugin_data.get(PLUGIN)).cloned().unwrap_or_default();
    Ok((w, widget_plugin(s, cmd, &plugin)?))
}

/// Call widget `w`'s plugin back with `event`, as the one undo step `label` (none: it joins the
/// step before it), and report.
fn fire(s: &mut Session, wp: &InstalledPlugin, w: NodeId, event: &Value, label: Option<&str>, fuel: Option<f64>) -> Result<Value> {
    let out = s.grouped_as(label, |s| callback(s, wp, w, event, fuel))?;
    report(s, w, out)
}

fn click(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "widget.click";
    let (w, wp) = widget_for(s, cmd, p)?;
    let target = string(p, "target").ok_or_else(|| bad(cmd, "`target` required (see widget.list)"))?;
    let all = targets(s.doc()?, w);
    if !all.contains(&target) {
        return Err(bad(cmd, format!("the widget has no `{target}` to click (it has: {})", all.join(", "))));
    }
    fire(s, &wp, w, &json!({"target": target}), Some(&wp.manifest.name), num(p, "fuel"))
}

fn set_text(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "widget.setText";
    let (w, wp) = widget_for(s, cmd, p)?;
    let name = string(p, "input").ok_or_else(|| bad(cmd, "`input` required (an editable text's name; see widget.list)"))?;
    let text = get(p, "text").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`text` required (the new text)"))?;
    let all = inputs(s.doc()?, w);
    let Some((_, input)) = all.iter().find(|(_, i)| i.name == name) else {
        let names: Vec<&str> = all.iter().map(|(_, i)| i.name.as_str()).collect();
        return Err(bad(cmd, format!("the widget has no text `{name}` to edit (it has: {})", names.join(", "))));
    };
    fire(s, &wp, w, &json!({"input": name, "text": input.clean(text)}), Some(&wp.manifest.name), num(p, "fuel"))
}

fn menu(s: &mut Session, p: &Value) -> Result<Value> {
    let w = widget_id(s, "widget.menu", p)?;
    let doc = s.doc()?;
    Ok(json!({"id": w, "plugin": doc.get(w)?.plugin_data.get(PLUGIN), "items": menu_of(doc, w)}))
}

fn menu_action(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "widget.menuAction";
    let (w, wp) = widget_for(s, cmd, p)?;
    let name = string(p, "item").ok_or_else(|| bad(cmd, "`item` required (a menu item's name; see widget.menu)"))?;
    let mut items = menu_of(s.doc()?, w);
    let Some(item) = items.iter_mut().find(|i| i.kind != MenuKind::Separator && i.name == name) else {
        let names: Vec<&str> = items.iter().filter(|i| i.kind != MenuKind::Separator).map(|i| i.name.as_str()).collect();
        return Err(bad(cmd, format!("the widget's menu has no `{name}` (it has: {})", names.join(", "))));
    };
    // A link is the app's to open (on a click only); the plugin isn't called.
    if item.kind == MenuKind::Link {
        let url = crate::dev_resources::check_url(&item.url).map_err(|e| bad(cmd, e))?;
        return Ok(json!({"id": w, "item": name, "url": url}));
    }
    let value = item.coerce(get(p, "value")).map_err(|e| bad(cmd, e))?;
    if !value.is_null() {
        item.value = Some(value.clone());
    }
    let (label, fuel) = (wp.manifest.name.clone(), num(p, "fuel"));
    let out = s.grouped(&label, |s| {
        // The menu shows the choice even when the plugin doesn't hand a new menu back.
        s.ds_mut()?.doc_mut().get_mut(w)?.plugin_data.insert(MENU.into(), json!(items).to_string());
        callback(s, &wp, w, &json!({"menu": name, "value": value}), fuel)
    })?;
    report(s, w, out)
}

fn fire_timer(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "widget.fireTimer";
    let (w, wp) = widget_for(s, cmd, p)?;
    if timer_of(s, w).is_none() {
        return Err(EngineError::Disabled(cmd.into(), "the widget has no timer pending (its plugin asks for one; see widget.list)".into()));
    }
    let ds = s.ds_mut()?;
    ds.widget_timers.retain(|t| t.widget != w);
    // A run of ticks is one undo step: a tick right after one joins it.
    let label = format!("{} timer", wp.manifest.name);
    let joins = s.undo_labels(1).first() == Some(&label);
    let r = fire(s, &wp, w, &json!({"timer": true}), (!joins).then_some(label.as_str()), num(p, "fuel"))?;
    if joins {
        s.ds_mut()?.history.redo.clear();
    }
    Ok(r)
}

/// Run widget command `cmd` (`widget.click`, `widget.setText`, `widget.menuAction` or
/// `widget.fireTimer`) on `doc`, a document that isn't one of the session's: the prototype
/// player's copy. The same callback as on the canvas, with the session's installed plugins and
/// the same sandbox limits, but the widget's state and layers are the copy's, nothing reaches the
/// session's documents and there is no undo step. The player keeps the clock for timers: the
/// result's `timerChanged` says whether the callback started or stopped the widget's timer, and
/// then `timer` is the delay it asked for in milliseconds (null: none); `widget.fireTimer` fires
/// whenever the player says the timer is due.
/// Returns the document as the callback left it (on failure the caller keeps its copy as it was)
/// and what the command returns.
pub fn run_on(s: &Session, doc: Document, cmd: &str, params: &Value) -> Result<(Document, Value)> {
    if !matches!(cmd, "widget.click" | "widget.setText" | "widget.menuAction" | "widget.fireTimer") {
        return Err(bad(cmd, "not a widget event"));
    }
    let mut scratch = Session::with_document(doc);
    scratch.prefs.plugins.clone_from(&s.prefs.plugins);
    scratch.plugins.modules.clone_from(&s.plugins.modules);
    // The timer the player holds for the widget, as the layer's stamp tells: due for
    // `widget.fireTimer`, else there only to see whether the callback replaces it.
    let held = id(params, "id").and_then(|i| widget_of(scratch.doc().ok()?, i)).and_then(|w| {
        let ds = scratch.ds_mut().ok()?;
        let stamp = ds.doc.node(w)?.plugin_data.get(TIMER)?.clone();
        ds.widget_timers.push(Timer { widget: w, due: Instant::now(), stamp: stamp.clone() });
        Some((w, stamp))
    });
    let mut out = scratch.execute(cmd, params)?;
    let kept = held.is_some_and(|(w, stamp)| cmd != "widget.fireTimer" && timer_of(&scratch, w).is_some_and(|t| t.stamp == stamp));
    out["timerChanged"] = json!(!kept);
    if kept {
        out["timer"] = Value::Null;
    }
    let ds = scratch.docs.pop().ok_or(EngineError::NoDocument)?;
    Ok((Arc::unwrap_or_clone(ds.doc), out))
}

/// Click `target` of the widget `id` is (or is in) on the prototype player's copy `doc` (see
/// [`run_on`]).
pub fn click_on(s: &Session, doc: Document, id: NodeId, target: &str) -> Result<(Document, Value)> {
    run_on(s, doc, "widget.click", &json!({"id": id, "target": target}))
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    let doc = s.doc()?;
    let widgets: Vec<Value> = doc
        .ids()
        .filter(|i| doc.node(*i).is_some_and(is_widget))
        .filter_map(|i| {
            let mut v = describe(s, i).ok()?;
            (v["name"], v["page"]) = (json!(doc.node(i)?.name), json!(doc.page_of(i)));
            Some(v)
        })
        .collect();
    Ok(json!({"widgets": widgets}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "widget.insert",
            "Insert widget",
            "{plugin (a widget plugin's id), x?, y? (top-left; default: centred in the view), fuel?} the plugin draws it; one undo step → {id, plugin, state, targets, inputs, menu, timer, output, logs, calls, fuel}",
            false,
            insert
        ),
        spec!(
            "widget.click",
            "Click widget",
            "{id? (a widget or a layer in one; default: the selected one), target (see widget.list), fuel?} calls the widget's plugin back, which redraws it; one undo step → like widget.insert",
            false,
            click
        ),
        spec!(
            "widget.setText",
            "Edit widget text",
            "{id? (a widget or a layer in one; default: the selected one), input (an editable text's name; see widget.list), text, fuel?} commits the text as editing it in place does (cut to its maximum length; one line unless it's multiline): the plugin is called back and redraws; one undo step → like widget.insert",
            false,
            set_text
        ),
        spec!(
            "widget.menu",
            "Widget property menu",
            "{id? (a widget or a layer in one; default: the selected one)} → {id, plugin, items: [{type: action|toggle|dropdown|color|separator|link, name, label, value, options, url}]}",
            false,
            menu
        ),
        spec!(
            "widget.menuAction",
            "Use widget menu item",
            "{id?, item (see widget.menu), value? (a toggle: true|false, default flipped; a dropdown or colour: one of its options), fuel?} calls the widget's plugin back; one undo step → like widget.insert. A link isn't opened: → {id, item, url}",
            false,
            menu_action
        ),
        spec!(
            "widget.fireTimer",
            "Fire widget timer",
            "{id?, fuel?} fires the widget's pending timer now (for scripts and tests; the app fires it when it's due): the plugin is called back; a run of timer callbacks is one undo step → like widget.insert",
            false,
            fire_timer
        ),
        spec!(
            "widget.list",
            "List widgets",
            "{} → {widgets: [{id, plugin, name, page, state, targets, inputs: [{name, text, maxLength, multiline}], menu, timer (milliseconds left, or null)}]}",
            false,
            list
        ),
    ]
}
