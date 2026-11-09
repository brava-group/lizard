//! MCP tool definitions (names, descriptions, JSON Schemas) and their implementations on top of a
//! [`Backend`]. Every tool reduces to control-channel methods, so headless and attached sessions
//! behave the same.

use base64::Engine as _;
use lizard_engine::params::normalize_paint;
use serde_json::{Map, Value, json};

use crate::backend::{Backend, decode_data, page_children};

/// The result of `tools/call`: MCP content blocks plus the `isError` flag.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(t: impl Into<String>) -> Self {
        Self { content: vec![json!({"type": "text", "text": t.into()})], is_error: false }
    }
    pub fn json(v: &Value) -> Self {
        Self::text(serde_json::to_string_pretty(v).unwrap_or_default())
    }
    pub fn error(t: impl Into<String>) -> Self {
        Self { content: vec![json!({"type": "text", "text": t.into()})], is_error: true }
    }
    /// An image block followed by a JSON text block describing it.
    pub fn image(bytes: &[u8], mime: &str, info: &Value) -> Self {
        Self {
            content: vec![
                json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes), "mimeType": mime}),
                json!({"type": "text", "text": info.to_string()}),
            ],
            is_error: false,
        }
    }
    pub fn to_value(&self) -> Value {
        json!({"content": self.content, "isError": self.is_error})
    }
    /// The first text block (tests and the CLI).
    pub fn first_text(&self) -> &str {
        self.content.iter().find_map(|c| c.get("text").and_then(Value::as_str)).unwrap_or("")
    }
}

// ---------- schemas ----------

fn num(desc: &str) -> Value {
    json!({"type": "number", "description": desc})
}
fn int(desc: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": desc})
}
fn string(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}
fn boolean(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}
fn id_schema(desc: &str) -> Value {
    json!({"description": desc, "anyOf": [{"type": "integer"}, {"type": "string"}]})
}
fn ids_schema(desc: &str) -> Value {
    json!({"type": "array", "description": desc, "items": {"anyOf": [{"type": "integer"}, {"type": "string"}]}})
}
fn obj(props: Value, required: &[&str]) -> Value {
    let mut o = json!({"type": "object", "properties": props});
    if !required.is_empty() {
        o["required"] = json!(required);
    }
    o
}

fn tool(name: &str, title: &str, desc: &str, schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": desc,
        "inputSchema": schema,
        "annotations": {"title": title, "readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false},
    })
}

fn online_tool(name: &str, title: &str, desc: &str, schema: Value, read_only: bool) -> Value {
    let mut definition = tool(name, title, desc, schema, read_only);
    definition["annotations"]["openWorldHint"] = json!(true);
    definition
}

/// Tool names, in `tools/list` order.
pub const TOOL_NAMES: &[&str] = &[
    "list_commands",
    "spawn_agents",
    "search_icons",
    "search_stock_photos",
    "place_stock_photo",
    "run_command",
    "batch",
    "create_node",
    "set_properties",
    "inspect_document",
    "get_node",
    "select",
    "export",
    "screenshot",
    "open_file",
    "save_file",
    "undo",
    "redo",
    "pointer",
    "key",
    "type_text",
    "inspect_ui",
];

/// All tools, in `tools/list` order.
pub fn tool_definitions() -> Vec<Value> {
    let empty = || obj(json!({}), &[]);
    vec![
        tool(
            "list_commands",
            "List commands",
            "List Lizard commands: id, label, menu path, shortcut, whether it is undoable, and a parameter description. Every menu item, panel action and shortcut is a command; run one with run_command (or several as one undo step with batch). Most commands act on `ids` when given, else on the selection. Use `filter` to narrow (e.g. \"align\", \"layout\", \"variable\", \"style\", \"component\", \"proto\").",
            obj(json!({"filter": string("Case-insensitive substring matched against id, label, menu path and parameter docs")}), &[]),
            true,
        ),
        tool(
            "spawn_agents",
            "Split design work",
            "Only the lead agent in an in-app parallel run can use this. Split work mode: if the user did not explicitly request multiple screens, create ONE screen with a shared shell and sibling section frames inside it. Assign one bounded section frame and a distinct task to each collaborator; continue other sections yourself. Pass exact shared colours, typography, spacing and icon family in every task. Never assign a whole second screen for an unspecified web app. Side by side mode is for complete alternative screens. Spawn one fewer agent than the selected count.",
            obj(
                json!({"config": {"type":"array","minItems":1,"maxItems":5,"items":obj(json!({"prompt":string("A brief, distinct design task"),"containerNodes":ids_schema("Existing frame IDs reserved for this agent")}), &["prompt","containerNodes"])}}),
                &["config"],
            ),
            false,
        ),
        tool(
            "search_icons",
            "Search UI icons",
            "Search the bundled Lucide, Feather, Phosphor and Material Symbols icon catalogs by English name. Use queries for up to 12 names in one call (default 4 matches each), or query for one name (default 12 matches). Give preferredLibrary to keep one visual family across a screen: matches from that family are returned first, and other families are used only when it has no match. The response contains exact library/name pairs for create_node {type: icon} or icon.create. A document's current preferred family is in the agent snapshot.",
            obj(
                json!({
                    "query": string("Icon meaning or name, for example home, settings, arrow-right, analytics"),
                    "queries": {"type": "array", "minItems": 1, "maxItems": 12, "items": {"type": "string"}, "description": "Up to 12 icon meanings to search together, e.g. [home, search, settings]. Use instead of query."},
                    "preferredLibrary": string("Preferred library, default lucide: lucide, feather, phosphor, material-symbols-outlined, material-symbols-rounded or material-symbols-sharp"),
                    "limit": int("Matches per query (single default 12, maximum 40; multiple default 4, maximum 12)"),
                }),
                &[],
            ),
            true,
        ),
        online_tool(
            "search_stock_photos",
            "Search Unsplash photos",
            "Search real photos on Unsplash for a design. Use a short, concrete subject query, then choose a photo ID from the results and call place_stock_photo. Requires LIZARD_UNSPLASH_ACCESS_KEY in the local app environment. Returns photographer and Unsplash attribution links along with preview URLs.",
            obj(
                json!({"query": string("One to three concrete words describing the desired photo"), "orientation": string("Optional: landscape, portrait or squarish")}),
                &["query"],
            ),
            true,
        ),
        online_tool(
            "place_stock_photo",
            "Place Unsplash photo",
            "Fetch a chosen Unsplash photo and place it on the canvas, or fill an existing shape with target. Choose the photo ID from search_stock_photos. Calls Unsplash's download tracking endpoint and stores the photographer, source and attribution links on the layer. Requires LIZARD_UNSPLASH_ACCESS_KEY in the local app environment.",
            obj(
                json!({"photoId": string("Unsplash photo ID from search_stock_photos"), "target": id_schema("Existing shape or frame to fill; omitted creates a new image layer"), "parent": id_schema("Parent frame for a new image layer"), "x": num("X of a new image layer"), "y": num("Y of a new image layer"), "width": num("Width of a new image layer"), "height": num("Height of a new image layer")}),
                &["photoId"],
            ),
            false,
        ),
        tool(
            "run_command",
            "Run command",
            "Execute one Lizard command by id with JSON params (ids and params from list_commands). Mutating commands are one undo step each; a failing command leaves the document unchanged. Examples: {\"command\":\"align.left\",\"params\":{\"ids\":[3,4]}}, {\"command\":\"layout.add\",\"params\":{\"ids\":[2],\"direction\":\"vertical\"}}, {\"command\":\"style.create\",\"params\":{\"type\":\"paint\",\"name\":\"Brand/Primary\",\"from\":5}}. Returns the command's result (creation commands return {\"id\"}). Programmatic calls never open dialogs.",
            obj(
                json!({"command": string("Command id, e.g. node.create, align.left, component.create, file.export"), "params": {"type": "object", "description": "Command parameters (see list_commands)"}}),
                &["command"],
            ),
            false,
        ),
        tool(
            "batch",
            "Run commands as one step",
            "Run several commands in order as ONE undo step (engine `command.batch`). If any command fails, everything is rolled back and the error names the failing step. Returns an array of the results in order; a separate text block may include asynchronous orchestrator feedback about an earlier section. A later step can use what an earlier step returned: name a step with \"as\": \"card\", then anywhere in a later step's params {\"$ref\": \"card\"} stands for the `id` in its result ({\"$step\": n} does the same by zero-based position). Add \"field\" for another part of the result: a key or a slash path, e.g. {\"$ref\": \"copy\", \"field\": \"ids/0\"} for the first id edit.duplicate returned. Any object with a `$ref` or `$step` key is a reference. Example: [{\"command\":\"node.create\",\"as\":\"card\",\"params\":{\"type\":\"frame\"}}, {\"command\":\"node.create\",\"params\":{\"type\":\"text\",\"text\":\"Hi\",\"parent\":{\"$ref\":\"card\"}}}] builds a frame and a text inside it in one call.",
            obj(
                json!({
                    "commands": {"type": "array", "minItems": 1, "description": "Steps to run", "items": obj(json!({"command": string("Command id"), "params": {"type": "object"}, "as": string("Name for this step, unique in the batch; later steps use its result with {\"$ref\": name}")}), &["command"])},
                    "label": string("Undo history label (default \"Batch\")"),
                }),
                &["commands"],
            ),
            false,
        ),
        tool(
            "create_node",
            "Create layer",
            "Create a layer (engine `node.create`) and return its id and inspected properties. Nested frames/components with no explicit fill are transparent; set props.fill for a visible card, panel or screen surface. For UI icons use type icon with library and icon from search_icons; keep the preferred library across a screen. Coordinates are page units, y down; x/y are relative to the parent frame (the page when there is none, so then they are page coordinates; groups don't count). Inside an auto-layout frame the position is decided by the layout. The new layer becomes the selection.",
            obj(
                json!({
                    "type": {"type": "string", "enum": ["frame", "rectangle", "ellipse", "polygon", "star", "line", "arrow", "text", "vector", "icon", "section", "component", "slice"], "description": "Layer type"},
                    "x": num("Left, relative to the parent frame (default 0)"),
                    "y": num("Top, relative to the parent frame (default 0)"),
                    "width": num("Width (default 100; text hugs its content unless width is set, which makes it wrap; for a vector with path, width/height scale the path, one alone keeping its proportion, and without them the path's own box is used)"),
                    "height": num("Height (default 100; ignored for lines/arrows and auto-height text)"),
                    "parent": id_schema("Parent frame/group/component id (default: the current page)"),
                    "index": int("Z-index among the parent's children (default: on top)"),
                    "name": string("Layer name"),
                    "text": string("Characters, for type text"),
                    "path": string("SVG path data, for type vector (e.g. \"M0 0 L100 0 L50 80 Z\")"),
                    "library": string("For type icon: the library ID returned by search_icons"),
                    "icon": string("For type icon: the exact icon name returned by search_icons"),
                    "color": string("For type icon: one colour for the vector paths, e.g. #334155"),
                    "props": {"type": "object", "description": "Any set_properties keys applied at creation, e.g. {\"fill\":\"#3366FF\",\"cornerRadius\":12,\"fontSize\":24,\"layout\":\"vertical\",\"gap\":8,\"padding\":16}"},
                }),
                &["type"],
            ),
            false,
        ),
        tool(
            "set_properties",
            "Set properties",
            "Set properties on one or more layers as one undo step (engine `node.set`). Keys: name, visible, locked, opacity (0-1), blend, x, y (relative to the parent frame; groups don't count), width, height, rotation (degrees), fill (colour) / fills (array of paints: \"#hex\" or {\"type\":\"solid\",\"color\":\"#hex\"} or {\"type\":\"linearGradient\",\"stops\":[\"#hex\",\"#hex\"]} or stops [{\"position\":0..1,\"color\":\"#hex\"}], optional from/to handles in the unit box; radialGradient too), stroke / strokes, strokeWeight, strokeAlign (inside|center|outside), cornerRadius (n or [tl,tr,br,bl]), effects (array, e.g. {\"type\":\"dropShadow\",\"color\":\"#00000040\",\"offset\":[0,4],\"radius\":8}), clip, layout (null|horizontal|vertical|grid), gap, padding (n or [t,r,b,l]), mainAlign (min|center|max|spaceBetween|spaceAround|spaceEvenly), crossAlign, wrap, contentAlign (auto|spaceBetween: wrapped lines), sizingH / sizingV (fixed|hug|fill), constraints {horizontal, vertical}, characters, fontFamily, fontStyle, fontSize, lineHeight, letterSpacing, textAlign, textAutoResize. Auto layout keys (gap, padding, mainAlign, crossAlign, wrap, …) apply to frames, components and instances only: other layers among the ids are skipped and named in `warnings`, and with none that can take them the call is an error. A fontStyle draws in the family's nearest face; `warnings` says when that face has another weight or slant than the name asks for. Run list_commands with filter \"node.set\" for the full list. Unknown keys are an error. Returns the updated layers.",
            obj(
                json!({"ids": ids_schema("Layer ids (default: the selection)"), "props": {"type": "object", "description": "Property values to set"}}),
                &["props"],
            ),
            false,
        ),
        tool(
            "inspect_document",
            "Inspect document",
            "The active document as JSON: documentId, name, path, dirty flag, currentPage, selection, the current page's layer tree (id, name, type, x/y relative to the parent frame (groups don't count), width, height, absoluteBounds in page coordinates, fills, strokes, effects, cornerRadius, layout, sizing, text properties, component links, overrides, bound variables), other pages summarised, styles, variable count and the undo history. Compare documentId and currentPage with the Lizard conversation snapshot before editing; use this tool to verify every change.",
            obj(
                json!({"depth": int("Tree depth for the current page (default 4)"), "allPages": boolean("Expand every page, not only the current one")}),
                &[],
            ),
            true,
        ),
        tool(
            "get_node",
            "Get layer",
            "One layer as JSON (same fields as inspect_document) with its children to `depth`.",
            obj(json!({"id": id_schema("Layer id"), "depth": int("Child depth (default 1)")}), &["id"]),
            true,
        ),
        tool(
            "select",
            "Select layers",
            "Replace the selection with `ids` (switching to their page). An empty array deselects everything. Most commands act on the selection when no `ids` are passed.",
            obj(json!({"ids": ids_schema("Layer ids to select")}), &["ids"]),
            false,
        ),
        tool(
            "export",
            "Export",
            "Export layers (default: the selection, else every top-level layer of the current page) as png, jpg, svg, webp or pdf at `scale`, or the whole file as `fig` (experimental: written from public format knowledge, with `warnings` for what was left out; other apps may not open every file) (PDF: with `bleed`, `cropMarks`, `bleedMarks` and `cmyk` for print; options `contentsOnly`, `includeBoundingBox`, and for SVG `svgOutlineText`, `svgIdAttribute`, `svgSimplifyStroke`). With `path` the file is written there (absolute path recommended) and the result says where. Without `path`: png/jpg/webp come back as an image content block, svg as text, other formats as base64 `data`.",
            obj(
                json!({
                    "ids": ids_schema("Layers to export (several are composited into one image)"),
                    "format": {"type": "string", "enum": ["png", "jpg", "svg", "webp", "pdf", "fig"], "description": "Format (default png, or from the path's extension)"},
                    "scale": num("Pixel scale for raster formats (default 1, max 64)"),
                    "size": string("Instead of `scale`: `2x`, or a width or height in pixels like `512w` / `512h`"),
                    "path": string("Output file path"),
                    "bleed": {"type": ["number", "string"], "description": "PDF: bleed around each page, in points or as a length like `3mm` or `0.125in`"},
                    "cropMarks": boolean("PDF: crop marks"),
                    "bleedMarks": boolean("PDF: bleed marks"),
                    "cmyk": boolean("PDF: CMYK colour instead of RGB"),
                    "contentsOnly": boolean("Only the layers themselves (default true); false also draws every other layer that shows in their area"),
                    "includeBoundingBox": boolean("Export the layers' own boxes, cutting off shadows and outside strokes (default false)"),
                    "svgOutlineText": boolean("SVG: text as outlines (default true); false keeps `<text>` elements"),
                    "svgIdAttribute": boolean("SVG: layer names as unique `id` attributes (default false)"),
                    "svgSimplifyStroke": boolean("SVG: strokes as `stroke` attributes where exact (default true); false writes outlines"),
                }),
                &[],
            ),
            true,
        ),
        tool(
            "screenshot",
            "Screenshot",
            "Look at the design: renders the given layers (or the whole current page, with its background) to PNG and returns it as image content, plus its size and the scale used. The image is scaled down to fit `maxSize` pixels. With `window: true` (or a window id from `inspect_ui`'s `windows`) and an attached app, captures that app window including the UI instead.",
            obj(
                json!({
                    "ids": ids_schema("Layers to render (default: the whole current page)"),
                    "scale": num("Requested scale (default 1; reduced to fit maxSize)"),
                    "maxSize": int("Largest side in pixels (default 1600)"),
                    "window": {"description": "Capture the app window: true for the one in front, or a window id (attached app only; headless renders the page)", "anyOf": [{"type": "boolean"}, {"type": "integer"}]},
                }),
                &[],
            ),
            true,
        ),
        tool(
            "open_file",
            "Open file",
            "Open a .lzd document (or legacy .lizard/.figcraft), .json document, .fig file (best effort), .svg or image as the active document. Returns the new document's summary.",
            obj(
                json!({
                    "path": string("File path"),
                    "variables": {"description": "REST API file JSON only: the local variables JSON (`GET /v1/files/:key/variables/local`) as a path or the JSON object. Default: `<name>.variables.json` or `variables.json` next to the file."},
                }),
                &["path"],
            ),
            false,
        ),
        tool(
            "save_file",
            "Save file",
            "Save the active document as a .lzd file. `path` is required the first time (an untitled document has no path); later saves reuse it. A `path` ending in .fig writes an experimental .fig export copy instead and leaves the document's own path and unsaved changes alone.",
            obj(json!({"path": string("Output .lzd path, or a .fig path for an experimental export copy")}), &[]),
            false,
        ),
        tool("undo", "Undo", "Undo the last change (one command, batch or gesture). Returns {undone: bool}.", empty(), false),
        tool("redo", "Redo", "Redo the last undone change. Returns {redone: bool}.", empty(), false),
        tool(
            "pointer",
            "Pointer gesture",
            "Drive the canvas with the mouse in page coordinates, exactly as a user would with the current tool: click to select, drag to move/resize/marquee, or draw with a shape tool (set `tool` first, e.g. {\"tool\":\"rectangle\",\"kind\":\"drag\",\"x\":0,\"y\":0,\"to\":[200,100]}). `double` on a text layer starts editing it. Returns the selection and tool.",
            obj(
                json!({
                    "kind": {"type": "string", "enum": ["click", "double", "drag", "down", "move", "up", "hover"], "description": "Gesture (default click)"},
                    "x": num("Page x"),
                    "y": num("Page y"),
                    "to": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "Drag end [x, y] (page)"},
                    "steps": int("Intermediate move events for drag (default 8)"),
                    "tool": {"type": "string", "description": "Tool to choose first: move|scale|hand|frame|section|slice|rectangle|line|arrow|ellipse|polygon|star|pen|pencil|text|comment"},
                    "shift": boolean("Shift held"),
                    "alt": boolean("Alt/Option held"),
                    "cmd": boolean("Cmd (macOS) / Ctrl held"),
                    "pressure": num("Pen pressure 0-1 (the Pencil records it as the stroke's width profile; a drag starts with it)"),
                    "pressureTo": num("Pen pressure 0-1 at a drag's end (interpolated along it)"),
                }),
                &["x", "y"],
            ),
            false,
        ),
        tool(
            "key",
            "Press key",
            "Press a key or shortcut as if typed on the canvas: a menu shortcut (e.g. {\"key\":\"G\",\"cmd\":true} groups, {\"key\":\"A\",\"alt\":true,\"shift\":false} …), a tool key (R, O, T, F, V…), Escape, Delete, Enter, arrows (nudge; shift = big nudge). While editing text, keys go to the text editor (Backspace, Delete, Left, Right, Home, End, Enter, Escape, SelectAll).",
            obj(
                json!({"key": string("Key name: a letter/digit, Escape, Delete, Backspace, Enter, ArrowLeft…"), "shift": boolean("Shift"), "alt": boolean("Alt/Option"), "cmd": boolean("Cmd/Ctrl")}),
                &["key"],
            ),
            false,
        ),
        tool(
            "type_text",
            "Type text",
            "Type into a text layer. With `id`, starts editing that layer with all of its text selected (so the typed text replaces it), types, then stops editing. Without `id`, types at the caret of the text currently being edited (after a double-click with pointer, or after drawing with the text tool).",
            obj(json!({"text": string("Text to type (\\n for a new line)"), "id": id_schema("Text layer to replace the contents of")}), &["text"]),
            false,
        ),
        tool(
            "inspect_ui",
            "Inspect UI",
            "UI state: current tool, view (zoom, centre), status, panels, timings and the app's windows (`windows`: id, title, files) when attached to the app; headless reports the session status.",
            empty(),
            true,
        ),
    ]
}

// ---------- helpers ----------

fn exec(b: &mut dyn Backend, cmd: &str, params: Value) -> Result<Value, String> {
    b.call("engine.execute", json!({"command": cmd, "params": params}))
}

fn args_obj(args: &Value) -> Result<Map<String, Value>, String> {
    match args {
        Value::Object(o) => Ok(o.clone()),
        Value::Null => Ok(Map::new()),
        _ => Err("`arguments` must be an object".into()),
    }
}

fn s_arg<'a>(a: &'a Map<String, Value>, k: &str) -> Result<Option<&'a str>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("`{k}` must be a string")),
    }
}

fn req_s<'a>(a: &'a Map<String, Value>, k: &str) -> Result<&'a str, String> {
    s_arg(a, k)?.ok_or_else(|| format!("`{k}` is required"))
}

fn n_arg(a: &Map<String, Value>, k: &str) -> Result<Option<f64>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => lizard_engine::params::f64_of(v).map(Some).ok_or_else(|| format!("`{k}` must be a finite number")),
    }
}

fn id_arg(v: &Value) -> Result<u64, String> {
    lizard_engine::params::id_of(v).map(|i| i.0).ok_or_else(|| format!("{v} is not a layer id"))
}

fn ids_arg(a: &Map<String, Value>, k: &str) -> Result<Option<Vec<u64>>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(arr)) => arr.iter().map(id_arg).collect::<Result<Vec<_>, _>>().map(Some),
        Some(v) => id_arg(v).map(|i| Some(vec![i])),
    }
}

fn png_info(bytes: &[u8]) -> (u32, u32) {
    crate::png_size(bytes).unwrap_or((0, 0))
}

/// Union of the layers' absolute bounds (page coordinates).
fn bounds_of(b: &mut dyn Backend, ids: &[u64]) -> Option<(f64, f64, f64, f64)> {
    let mut acc: Option<(f64, f64, f64, f64)> = None;
    for id in ids.iter().take(500) {
        let Ok(n) = b.call("node.get", json!({"id": id, "depth": 0})) else { continue };
        let Some(ab) = n.get("absoluteBounds") else { continue };
        let g = |k: &str| ab.get(k).and_then(Value::as_f64);
        let (Some(x), Some(y), Some(w), Some(h)) = (g("x"), g("y"), g("width"), g("height")) else { continue };
        acc = Some(match acc {
            None => (x, y, x + w, y + h),
            Some((a, b2, c, d)) => (a.min(x), b2.min(y), c.max(x + w), d.max(y + h)),
        });
    }
    acc
}

/// Make paint/effect JSON friendlier than the engine's strict serde (see
/// [`lizard_engine::params::normalize_paint`]). The engine reads paints this way itself; effects
/// still need it here.
pub fn normalize_props(props: &mut Map<String, Value>) {
    for k in ["fills", "strokes", "effects"] {
        if let Some(v) = props.get_mut(k) {
            normalize_paint(v);
        }
    }
}

/// [`normalize_props`] for command params: `props`, flat node.set keys, `paint`, `effect`.
fn friendly_params(p: &mut Value) {
    let Some(o) = p.as_object_mut() else { return };
    if let Some(Value::Object(props)) = o.get_mut("props") {
        normalize_props(props);
    }
    normalize_props(o);
    for k in ["paint", "effect"] {
        if let Some(v) = o.get_mut(k) {
            normalize_paint(v);
        }
    }
}

/// The model uses nested frames as layout containers. An omitted paint on those frames means
/// transparent; the white document/screen background must be requested explicitly.
fn transparent_nested_container(p: &mut Value) {
    let Some(params) = p.as_object_mut() else { return };
    let nested = params.get("parent").is_some_and(|parent| !parent.is_null());
    let container =
        params.get("type").and_then(Value::as_str).is_some_and(|kind| matches!(kind.to_ascii_lowercase().as_str(), "frame" | "component"));
    if !nested || !container {
        return;
    }
    let props = params.entry("props").or_insert_with(|| json!({}));
    if props.is_null() {
        *props = json!({});
    }
    let Some(props) = props.as_object_mut() else { return };
    if !["fill", "fills", "fillStyle"].iter().any(|key| props.contains_key(*key)) {
        props.insert("fills".into(), json!([]));
    }
}

// ---------- dispatch ----------

/// Run one tool. Every failure becomes an `isError` result (never a panic).
pub fn call_tool(b: &mut dyn Backend, name: &str, args: &Value) -> ToolResult {
    let a = match args_obj(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(e),
    };
    let relevant = matches!(
        name,
        "create_node" | "set_properties" | "place_stock_photo" | "get_node" | "inspect_document" | "screenshot" | "select" | "run_command" | "batch"
    );
    if relevant {
        let ids = activity_ids(name, args);
        let position = if name == "create_node" {
            let x = args.get("x").and_then(Value::as_f64);
            let y = args.get("y").and_then(Value::as_f64);
            match (x, y) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
                    let width = args.get("width").and_then(Value::as_f64).filter(|size| size.is_finite()).unwrap_or(0.0);
                    let height = args.get("height").and_then(Value::as_f64).filter(|size| size.is_finite()).unwrap_or(0.0);
                    let parent = args.get("parent").and_then(lizard_engine::params::id_of).map(|id| id.0);
                    Some(json!({"x": x + width * 0.35, "y": y + height * 0.35, "parent": parent}))
                }
                _ => None,
            }
        } else {
            None
        };
        let arguments = (name == "create_node").then(|| {
            let mut preview = Map::new();
            for key in ["type", "x", "y", "width", "height", "parent", "color"] {
                if let Some(value) = args.get(key) {
                    preview.insert(key.into(), value.clone());
                }
            }
            if let Some(text) = args.get("text").and_then(Value::as_str) {
                preview.insert("text".into(), json!(text.chars().take(200).collect::<String>()));
            }
            if let Some(props) = args.get("props").and_then(Value::as_object) {
                let mut preview_props = Map::new();
                for key in ["fill", "fontSize", "cornerRadius"] {
                    if let Some(value) = props.get(key) {
                        preview_props.insert(key.into(), value.clone());
                    }
                }
                preview.insert("props".into(), Value::Object(preview_props));
            }
            Value::Object(preview)
        });
        b.agent_activity(json!({"phase":"start", "tool":name, "ids":ids, "position":position, "arguments":arguments}));
    }
    let r = match name {
        "list_commands" => list_commands(b, &a),
        "search_icons" => search_icons(&a),
        "search_stock_photos" => crate::stock::search(b, &a),
        "place_stock_photo" => crate::stock::place(b, &a),
        "run_command" => run_command(b, &a),
        "batch" => batch(b, &a),
        "create_node" => create_node(b, &a),
        "set_properties" => set_properties(b, &a),
        "inspect_document" => inspect_document(b, &a),
        "get_node" => get_node(b, &a),
        "select" => select(b, &a),
        "export" => export(b, &a),
        "screenshot" => screenshot(b, &a),
        "open_file" => open_file(b, &a),
        "save_file" => save_file(b, &a),
        "undo" => exec(b, "edit.undo", json!({})).map(|v| ToolResult::json(&v)),
        "redo" => exec(b, "edit.redo", json!({})).map(|v| ToolResult::json(&v)),
        "pointer" => pointer(b, &a),
        "key" => key(b, &a),
        "type_text" => type_text(b, &a),
        "inspect_ui" => b.call("ui.inspect", json!({})).map(|v| ToolResult::json(&v)),
        "spawn_agents" => b.call("agent.spawn", args.clone()).map(|v| ToolResult::json(&v)),
        other => Err(format!("unknown tool `{other}` (tools/list has: {})", TOOL_NAMES.join(", "))),
    };
    let result = r.unwrap_or_else(ToolResult::error);
    if relevant {
        let mut ids = activity_ids(name, args);
        if name == "batch"
            && !result.is_error
            && let Ok(completed) = serde_json::from_str::<Value>(result.first_text())
            && let Some(steps) = completed.as_array()
        {
            ids = steps.iter().rev().find_map(|step| step.get("id").and_then(Value::as_u64)).into_iter().collect();
        }
        if name == "create_node"
            && !result.is_error
            && let Ok(created) = serde_json::from_str::<Value>(result.first_text())
            && let Some(id) = created.get("id").and_then(Value::as_u64)
        {
            ids = vec![id];
        }
        let preview = if name == "screenshot" && !result.is_error {
            result
                .content
                .iter()
                .find(|block| block.get("type").and_then(Value::as_str) == Some("image"))
                .and_then(|block| block.get("data").and_then(Value::as_str))
                .filter(|data| data.len() <= 2_000_000)
        } else {
            None
        };
        let parent = (name == "create_node").then(|| args.get("parent").and_then(lizard_engine::params::id_of).map(|id| id.0)).flatten();
        b.agent_activity(json!({"phase": if result.is_error {"failed"} else {"done"}, "tool":name, "ids":ids, "parent":parent, "preview":preview}));
    }
    result
}

fn activity_ids(name: &str, args: &Value) -> Vec<u64> {
    if name == "create_node" {
        return args.get("parent").and_then(lizard_engine::params::id_of).map(|id| id.0).into_iter().collect();
    }
    if !matches!(name, "set_properties" | "get_node" | "screenshot" | "select" | "run_command" | "batch") {
        return Vec::new();
    }
    let source = if name == "run_command" { &args["params"] } else { args };
    let mut ids: Vec<u64> =
        source.get("ids").and_then(Value::as_array).into_iter().flatten().filter_map(lizard_engine::params::id_of).map(|id| id.0).take(8).collect();
    if let Some(id) = source.get("id").and_then(lizard_engine::params::id_of) {
        ids.push(id.0);
    }
    ids.truncate(8);
    ids
}

fn list_commands(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let filter = s_arg(a, "filter")?.map(str::to_lowercase).unwrap_or_default();
    let all = b.call("engine.commands", json!({}))?;
    let list: Vec<Value> = all
        .as_array()
        .map(|v| {
            v.iter()
                .filter(|c| {
                    filter.is_empty()
                        || ["id", "label", "menu", "params"]
                            .iter()
                            .any(|k| c.get(*k).and_then(Value::as_str).is_some_and(|s| s.to_lowercase().contains(&filter)))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if list.is_empty() {
        return Ok(ToolResult::text(format!("no command matches `{filter}`")));
    }
    // One compact line per command keeps this readable for agents.
    let mut out = format!("{} commands (id — label [menu] (shortcut) params; * = undoable):\n", list.len());
    for c in &list {
        let g = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
        out.push_str(g("id"));
        if c.get("undoable").and_then(Value::as_bool) == Some(true) {
            out.push('*');
        }
        out.push_str(" — ");
        out.push_str(g("label"));
        if !g("menu").is_empty() {
            out.push_str(&format!(" [{}]", g("menu")));
        }
        if !g("shortcut").is_empty() {
            out.push_str(&format!(" ({})", g("shortcut")));
        }
        out.push(' ');
        out.push_str(g("params"));
        out.push('\n');
    }
    Ok(ToolResult::text(out))
}

fn search_icons(a: &Map<String, Value>) -> Result<ToolResult, String> {
    let preferred = Some(s_arg(a, "preferredLibrary")?.unwrap_or("lucide"));
    if let Some(queries) = a.get("queries").filter(|value| !value.is_null()) {
        if a.get("query").is_some_and(|value| !value.is_null()) {
            return Err("use either `query` or `queries`".into());
        }
        let queries = queries.as_array().filter(|queries| (1..=12).contains(&queries.len())).ok_or("`queries` must contain 1 to 12 names")?;
        let limit = match a.get("limit") {
            None | Some(Value::Null) => 4,
            Some(value) => value.as_u64().filter(|limit| (1..=12).contains(limit)).ok_or("`limit` must be between 1 and 12 for `queries`")? as usize,
        };
        let mut searches = Vec::with_capacity(queries.len());
        for query in queries {
            let query = query.as_str().filter(|query| !query.trim().is_empty()).ok_or("each `queries` entry must be a non-empty string")?;
            searches.push(json!({"query": query, "search": lizard_engine::icon_catalog::search(query, preferred, limit)?}));
        }
        return Ok(ToolResult::json(&json!({"searches": searches})));
    }
    let query = req_s(a, "query")?;
    let limit = match a.get("limit") {
        None | Some(Value::Null) => 12,
        Some(value) => value.as_u64().filter(|limit| (1..=40).contains(limit)).ok_or("`limit` must be between 1 and 40")? as usize,
    };
    let result = lizard_engine::icon_catalog::search(query, preferred, limit)?;
    let families = lizard_engine::icon_catalog::counts()?;
    Ok(ToolResult::json(
        &json!({"search": result, "libraries": families.iter().map(|(name, count)| json!({"id": name, "count": count})).collect::<Vec<_>>() }),
    ))
}

fn run_command(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let cmd = req_s(a, "command")?;
    let params = match a.get("params") {
        None | Some(Value::Null) => json!({}),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err("`params` must be an object".into()),
    };
    let mut params = params;
    friendly_params(&mut params);
    if cmd == "node.create" {
        transparent_nested_container(&mut params);
    }
    exec(b, cmd, params).map(|v| ToolResult::json(&v))
}

fn batch(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let Some(Value::Array(cmds)) = a.get("commands") else { return Err("`commands` must be an array of {command, params}".into()) };
    if cmds.is_empty() {
        return Err("`commands` is empty".into());
    }
    let mut steps = Vec::with_capacity(cmds.len());
    for (i, c) in cmds.iter().enumerate() {
        let Some(cmd) = c.get("command").and_then(Value::as_str) else { return Err(format!("step {i}: `command` must be a string")) };
        let params = match c.get("params") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => return Err(format!("step {i} ({cmd}): `params` must be an object")),
        };
        let mut params = params;
        friendly_params(&mut params);
        if cmd == "node.create" {
            transparent_nested_container(&mut params);
        }
        let mut step = json!({"command": cmd, "params": params});
        if let (Some(name), Some(o)) = (c.get("as"), step.as_object_mut()) {
            o.insert("as".into(), name.clone());
        }
        steps.push(step);
    }
    let label = s_arg(a, "label")?.unwrap_or("Batch");
    if b.has_ui() {
        let started = b.call("agent.batch.begin", json!({"label": label, "commands": steps}));
        if matches!(started, Ok(ref result) if result.get("accepted").and_then(Value::as_bool) == Some(true)) {
            for _ in 0..steps.len() {
                if let Err(error) = b.call("agent.batch.step", json!({})) {
                    let _ = b.call("agent.batch.cancel", json!({}));
                    return Err(error);
                }
                // The control request runs at the start of a frame. Give the canvas time to
                // paint this real node before requesting the next one.
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            return b.call("agent.batch.finish", json!({})).map(|result| with_review_feedback(b, ToolResult::json(&result)));
        }
    }
    let result = exec(b, "command.batch", json!({"label": label, "commands": steps}))?;
    if b.has_ui()
        && let Some(completed) = result.as_array()
        && let Some(id) = completed.iter().rev().find_map(|step| step.get("id").and_then(Value::as_u64))
    {
        // An atomic batch has already finished. Point at its last created node rather than
        // replaying earlier cursor positions after the work is done.
        b.agent_activity(json!({"phase":"done","tool":"create_node","ids":[id]}));
    }
    Ok(with_review_feedback(b, ToolResult::json(&result)))
}

fn with_review_feedback(b: &mut dyn Backend, mut result: ToolResult) -> ToolResult {
    if b.has_ui()
        && let Ok(Value::Array(feedback)) = b.call("agent.review.take", json!({}))
    {
        for note in feedback.into_iter().filter_map(|item| item.as_str().map(str::to_owned)).take(3) {
            result.content.push(json!({"type":"text","text":format!("Orchestrator review of an earlier section: {note}\nContinue building the current section. Correct the cited nodes when you reach a safe batch boundary; do not redraw the entire screen.")}));
        }
    }
    result
}

fn create_node(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    req_s(a, "type")?;
    for k in ["x", "y", "width", "height"] {
        n_arg(a, k)?;
    }
    if let Some(p) = a.get("props")
        && !p.is_object()
        && !p.is_null()
    {
        return Err("`props` must be an object".into());
    }
    let mut params = Map::new();
    for k in [
        "type", "x", "y", "width", "height", "parent", "index", "name", "text", "path", "points", "closed", "library", "icon", "color", "props",
        "select",
    ] {
        if let Some(v) = a.get(k).filter(|v| !v.is_null()) {
            params.insert(k.into(), v.clone());
        }
    }
    if let Some(Value::Object(p)) = params.get_mut("props") {
        normalize_props(p);
    }
    let mut params = Value::Object(params);
    transparent_nested_container(&mut params);
    let r = exec(b, "node.create", params)?;
    let id = r.get("id").cloned().ok_or("node.create returned no id")?;
    let node = b.call("node.get", json!({"id": id, "depth": 0}))?;
    Ok(ToolResult::json(&json!({"id": id, "node": node})))
}

fn set_properties(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let props = match a.get("props") {
        Some(Value::Object(p)) if !p.is_empty() => p.clone(),
        _ => return Err("`props` must be a non-empty object".into()),
    };
    let mut props = props;
    normalize_props(&mut props);
    let mut params = json!({"props": props});
    let ids = ids_arg(a, "ids")?;
    if let Some(ids) = &ids {
        if ids.is_empty() {
            return Err("`ids` is empty".into());
        }
        params["ids"] = json!(ids);
    }
    let warnings = exec(b, "node.set", params)?.get("warnings").cloned();
    let ids = match ids {
        Some(i) => i,
        None => b
            .call("document.inspect", json!({"depth": 0}))?
            .get("selection")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default(),
    };
    let nodes: Vec<Value> = ids.iter().take(20).filter_map(|i| b.call("node.get", json!({"id": i, "depth": 0})).ok()).collect();
    let mut out = json!({"ok": true, "nodes": nodes});
    if let Some(w) = warnings {
        out["warnings"] = w;
    }
    Ok(ToolResult::json(&out))
}

fn inspect_document(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let depth = n_arg(a, "depth")?.unwrap_or(4.0).clamp(0.0, 64.0) as u64;
    let all = match a.get("allPages") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(x)) => *x,
        Some(_) => return Err("`allPages` must be a boolean".into()),
    };
    b.call("document.inspect", json!({"depth": depth, "allPages": all})).map(|v| with_review_feedback(b, ToolResult::json(&v)))
}

fn get_node(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let id = id_arg(a.get("id").ok_or("`id` is required")?)?;
    let depth = n_arg(a, "depth")?.unwrap_or(1.0).clamp(0.0, 64.0) as u64;
    let v = b.call("node.get", json!({"id": id, "depth": depth}))?;
    if v.is_null() {
        return Err(format!("no such layer {id}"));
    }
    Ok(ToolResult::json(&v))
}

fn select(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let ids = ids_arg(a, "ids")?.ok_or("`ids` is required (an empty array deselects)")?;
    if ids.is_empty() {
        exec(b, "select.none", json!({}))?;
    } else {
        exec(b, "select.set", json!({"ids": ids}))?;
    }
    let sel = b.call("document.inspect", json!({"depth": 0}))?.get("selection").cloned().unwrap_or(Value::Null);
    Ok(ToolResult::json(&json!({"selection": sel})))
}

fn export(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let mut params = Map::new();
    if let Some(ids) = ids_arg(a, "ids")? {
        params.insert("ids".into(), json!(ids));
    }
    let format = s_arg(a, "format")?.map(str::to_ascii_lowercase);
    if let Some(f) = &format {
        if !["png", "jpg", "jpeg", "svg", "webp", "pdf", "fig"].contains(&f.as_str()) {
            return Err(format!("unknown format `{f}` (png|jpg|svg|webp|pdf|fig)"));
        }
        params.insert("format".into(), json!(f));
    }
    if let Some(s) = n_arg(a, "scale")? {
        if s <= 0.0 {
            return Err("`scale` must be positive".into());
        }
        params.insert("scale".into(), json!(s));
    }
    if let Some(s) = s_arg(a, "size")? {
        params.insert("size".into(), json!(s));
    }
    // PDF print options and the export options, checked by the command.
    for k in
        ["bleed", "cropMarks", "bleedMarks", "cmyk", "contentsOnly", "includeBoundingBox", "svgOutlineText", "svgIdAttribute", "svgSimplifyStroke"]
    {
        if let Some(v) = a.get(k).filter(|v| !v.is_null()) {
            params.insert(k.into(), v.clone());
        }
    }
    if let Some(p) = s_arg(a, "path")? {
        params.insert("path".into(), json!(p));
        return exec(b, "file.export", Value::Object(params)).map(|v| ToolResult::json(&v));
    }
    let r = exec(b, "file.export", Value::Object(params))?;
    let fmt = r.get("format").and_then(Value::as_str).unwrap_or("png").to_string();
    let bytes = decode_data(&r)?;
    match fmt.as_str() {
        "png" => {
            let (w, h) = png_info(&bytes);
            Ok(ToolResult::image(&bytes, "image/png", &json!({"format": "png", "width": w, "height": h, "bytes": bytes.len()})))
        }
        "jpg" | "jpeg" => Ok(ToolResult::image(&bytes, "image/jpeg", &json!({"format": "jpg", "bytes": bytes.len()}))),
        "webp" => Ok(ToolResult::image(&bytes, "image/webp", &json!({"format": "webp", "bytes": bytes.len()}))),
        "svg" => Ok(ToolResult::text(String::from_utf8_lossy(&bytes).into_owned())),
        other => {
            let mut out = json!({"format": other, "bytes": bytes.len(), "data": base64::engine::general_purpose::STANDARD.encode(&bytes)});
            if let Some(w) = r.get("warnings") {
                out["warnings"] = w.clone();
            }
            Ok(ToolResult::json(&out))
        }
    }
}

fn screenshot(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let id = a.get("window").and_then(Value::as_u64);
    if (id.is_some() || matches!(a.get("window"), Some(Value::Bool(true)))) && b.has_ui() {
        let bytes = match id {
            Some(id) => b.window_png(id)?,
            None => b.screenshot_png()?,
        };
        let (w, h) = png_info(&bytes);
        return Ok(with_review_feedback(b, ToolResult::image(&bytes, "image/png", &json!({"source": "window", "width": w, "height": h}))));
    }
    let max = n_arg(a, "maxSize")?.unwrap_or(1600.0).clamp(16.0, 8192.0);
    let req = n_arg(a, "scale")?.unwrap_or(1.0);
    if req <= 0.0 {
        return Err("`scale` must be positive".into());
    }
    let ids = ids_arg(a, "ids")?.unwrap_or_default();
    let measured = if ids.is_empty() { page_children(b)? } else { ids.clone() };
    let mut scale = req;
    if let Some((x0, y0, x1, y1)) = bounds_of(b, &measured) {
        let side = (x1 - x0).max(y1 - y0).max(1e-6);
        scale = scale.min(max / side);
    }
    let mut bytes = b.render_png(&ids, scale)?;
    let (mut w, mut h) = png_info(&bytes);
    // Layer bounds leave out effects (shadows) and overflow of unclipped frames, but the render
    // includes them: shrink to the measured image until it fits `maxSize` (rounding can add a pixel).
    for _ in 0..4 {
        let side = f64::from(w.max(h));
        if side <= max {
            break;
        }
        scale *= max / side;
        bytes = b.render_png(&ids, scale)?;
        (w, h) = png_info(&bytes);
    }
    Ok(with_review_feedback(
        b,
        ToolResult::image(
            &bytes,
            "image/png",
            &json!({"source": if ids.is_empty() { "page" } else { "layers" }, "ids": ids, "scale": scale, "width": w, "height": h}),
        ),
    ))
}

fn open_file(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let path = req_s(a, "path")?;
    let mut params = json!({"path": path});
    match a.get("variables") {
        None | Some(Value::Null) => {}
        Some(v @ (Value::String(_) | Value::Object(_))) => params["variables"] = v.clone(),
        Some(_) => return Err("`variables` must be a path or the local variables JSON object".into()),
    }
    let r = exec(b, "file.open", params)?;
    let doc = b.call("document.inspect", json!({"depth": 1}))?;
    Ok(ToolResult::json(&json!({"opened": r, "document": doc})))
}

fn save_file(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let params = match s_arg(a, "path")? {
        Some(p) => json!({"path": p}),
        None => json!({}),
    };
    exec(b, "file.save", params).map(|v| ToolResult::json(&v))
}

fn mods(a: &Map<String, Value>, out: &mut Map<String, Value>) -> Result<(), String> {
    for k in ["shift", "alt", "cmd", "ctrl"] {
        match a.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(x)) => {
                out.insert(k.into(), json!(x));
            }
            Some(_) => return Err(format!("`{k}` must be a boolean")),
        }
    }
    Ok(())
}

fn pointer(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let x = n_arg(a, "x")?.ok_or("`x` is required")?;
    let y = n_arg(a, "y")?.ok_or("`y` is required")?;
    let kind = s_arg(a, "kind")?.unwrap_or("click");
    if let Some(t) = s_arg(a, "tool")? {
        exec(b, "tool.set", json!({"tool": t}))?;
    }
    let mut p = Map::new();
    p.insert("kind".into(), json!(kind));
    p.insert("x".into(), json!(x));
    p.insert("y".into(), json!(y));
    if let Some(to) = a.get("to").filter(|v| !v.is_null()) {
        let arr =
            to.as_array().filter(|t| t.len() == 2 && t.iter().all(|v| lizard_engine::params::f64_of(v).is_some())).ok_or("`to` must be [x, y]")?;
        p.insert("to".into(), Value::Array(arr.clone()));
    }
    if let Some(s) = n_arg(a, "steps")? {
        p.insert("steps".into(), json!(s.clamp(1.0, 200.0) as u64));
    }
    for k in ["pressure", "pressureTo"] {
        if let Some(v) = n_arg(a, k)? {
            p.insert(k.into(), json!(v.clamp(0.0, 1.0)));
        }
    }
    mods(a, &mut p)?;
    b.call("ui.pointer", Value::Object(p)).map(|v| ToolResult::json(&v))
}

fn key(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let k = req_s(a, "key")?;
    let mut p = Map::new();
    p.insert("key".into(), json!(k));
    mods(a, &mut p)?;
    b.call("ui.key", Value::Object(p)).map(|v| ToolResult::json(&v))
}

fn type_text(b: &mut dyn Backend, a: &Map<String, Value>) -> Result<ToolResult, String> {
    let text = req_s(a, "text")?;
    match a.get("id").filter(|v| !v.is_null()) {
        Some(id) => {
            let id = id_arg(id)?;
            exec(b, "text.edit.begin", json!({"id": id}))?;
            let typed = b.call("ui.type", json!({"text": text}));
            // Always leave edit mode, even when typing failed.
            let ended = exec(b, "text.edit.end", json!({}));
            typed?;
            ended?;
            let node = b.call("node.get", json!({"id": id, "depth": 0}))?;
            Ok(ToolResult::json(&json!({"ok": true, "node": node})))
        }
        None => b.call("ui.type", json!({"text": text})).map(|v| ToolResult::json(&v)),
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use crate::Headless;

    #[test]
    fn completed_review_reaches_the_designers_next_tool_result() {
        struct Reviewed;
        impl Backend for Reviewed {
            fn call(&mut self, method: &str, _: Value) -> Result<Value, String> {
                if method == "agent.review.take" { Ok(json!(["Frame 12 has an unintended white fill."])) } else { Err(method.into()) }
            }
            fn has_ui(&self) -> bool {
                true
            }
            fn describe(&self) -> String {
                "reviewed".into()
            }
        }
        let result = with_review_feedback(&mut Reviewed, ToolResult::json(&json!([{"id":12}])));
        assert_eq!(result.content.len(), 2);
        assert!(result.content[1]["text"].as_str().is_some_and(|text| text.contains("Frame 12")));
    }

    #[test]
    fn nested_agent_frames_default_to_transparent_without_erasing_explicit_paint() {
        let mut backend = Headless::new();
        let result = call_tool(
            &mut backend,
            "batch",
            &json!({"commands":[
                {"command":"node.create","as":"screen","params":{"type":"frame","name":"Screen","props":{"fill":"#101828"}}},
                {"command":"node.create","params":{"type":"frame","name":"Layout","parent":{"$ref":"screen"}}},
                {"command":"node.create","params":{"type":"frame","name":"Card","parent":{"$ref":"screen"},"props":{"fill":"#FFFFFF"}}}
            ]}),
        );
        assert!(!result.is_error, "{}", result.first_text());
        let steps: Value = serde_json::from_str(result.first_text()).unwrap();
        let layout = backend.call("node.get", json!({"id":steps[1]["id"]})).unwrap();
        let card = backend.call("node.get", json!({"id":steps[2]["id"]})).unwrap();
        assert!(layout.get("fills").is_none_or(|fills| fills.as_array().is_some_and(Vec::is_empty)), "{layout}");
        assert_eq!(card["fills"].as_array().map(Vec::len), Some(1));
    }

    struct Traced {
        backend: Headless,
        activity: Vec<Value>,
    }

    impl Backend for Traced {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            self.backend.call(method, params)
        }
        fn has_ui(&self) -> bool {
            false
        }
        fn describe(&self) -> String {
            "traced headless".into()
        }
        fn agent_activity(&mut self, activity: Value) {
            self.activity.push(activity);
        }
    }

    #[test]
    fn atomic_batch_reports_created_nodes_for_the_agent_cursor() {
        let mut traced = Traced { backend: Headless::new(), activity: Vec::new() };
        let result = call_tool(
            &mut traced,
            "batch",
            &json!({"commands":[
                {"command":"node.create","params":{"type":"frame","name":"Screen"}},
                {"command":"node.create","params":{"type":"frame","name":"Section"}}
            ]}),
        );
        assert!(!result.is_error, "{}", result.first_text());
        let done = traced.activity.iter().find(|activity| activity["phase"] == "done").unwrap();
        assert_eq!(done["ids"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn attached_atomic_batch_points_to_the_last_created_node_without_replaying_work() {
        struct VisibleAtomic {
            activity: Vec<Value>,
        }
        impl Backend for VisibleAtomic {
            fn call(&mut self, method: &str, _: Value) -> Result<Value, String> {
                match method {
                    "agent.batch.begin" => Ok(json!({"accepted":false})),
                    "engine.execute" => Ok(json!([{"id":10},{"id":11}])),
                    _ => Err(format!("unexpected {method}")),
                }
            }
            fn has_ui(&self) -> bool {
                true
            }
            fn describe(&self) -> String {
                "visible atomic test".into()
            }
            fn agent_activity(&mut self, activity: Value) {
                self.activity.push(activity);
            }
        }
        let mut backend = VisibleAtomic { activity: Vec::new() };
        let result = call_tool(&mut backend, "batch", &json!({"commands":[{"command":"node.create","params":{"type":"frame"}}]}));
        assert!(!result.is_error, "{}", result.first_text());
        let visited: Vec<_> = backend
            .activity
            .iter()
            .filter(|activity| activity["tool"] == "create_node" && activity["phase"] == "done")
            .map(|activity| activity["ids"][0].clone())
            .collect();
        assert_eq!(visited, [json!(11)]);
    }

    #[test]
    fn attached_batch_advances_one_control_request_per_step() {
        struct VisibleBatch {
            methods: Vec<String>,
        }
        impl Backend for VisibleBatch {
            fn call(&mut self, method: &str, _: Value) -> Result<Value, String> {
                self.methods.push(method.into());
                match method {
                    "agent.batch.begin" => Ok(json!({"accepted":true})),
                    "agent.batch.step" => Ok(json!({"done":false})),
                    "agent.batch.finish" => Ok(json!([{"id":2},{"id":3}])),
                    _ => Err(format!("unexpected {method}")),
                }
            }
            fn has_ui(&self) -> bool {
                true
            }
            fn describe(&self) -> String {
                "visible test".into()
            }
        }
        let mut backend = VisibleBatch { methods: Vec::new() };
        let args = json!({"commands":[
            {"command":"node.create","params":{"type":"frame"}},
            {"command":"node.create","params":{"type":"text","text":"Hi"}}
        ]});
        let result = call_tool(&mut backend, "batch", &args);
        assert!(!result.is_error, "{}", result.first_text());
        assert_eq!(backend.methods, ["agent.batch.begin", "agent.batch.step", "agent.batch.step", "agent.batch.finish", "agent.review.take"]);
    }

    #[test]
    fn activity_targets_only_explicit_design_nodes() {
        assert_eq!(activity_ids("get_node", &json!({"id":42})), vec![42]);
        assert_eq!(activity_ids("set_properties", &json!({"ids":[2,3]})), vec![2, 3]);
        assert_eq!(activity_ids("create_node", &json!({"parent":7,"name":"Demo"})), vec![7]);
        assert_eq!(activity_ids("create_node", &json!({"parent":"7","name":"Demo"})), vec![7]);
        assert_eq!(activity_ids("run_command", &json!({"command":"node.set","params":{"ids":[9]}})), vec![9]);
        assert!(activity_ids("list_commands", &json!({"ids":[5]})).is_empty());
    }

    #[test]
    fn creation_reports_planned_position_then_created_id() {
        let mut traced = Traced { backend: Headless::new(), activity: Vec::new() };
        let result = call_tool(
            &mut traced,
            "create_node",
            &json!({
                "type":"rectangle", "x":100, "y":80, "width":200, "height":100
            }),
        );
        assert!(!result.is_error, "{}", result.first_text());
        assert_eq!(traced.activity.len(), 2);
        assert_eq!(traced.activity[0]["phase"], "start");
        assert_eq!(traced.activity[0]["position"]["x"], 170.0);
        assert_eq!(traced.activity[0]["position"]["y"], 115.0);
        assert_eq!(traced.activity[0]["arguments"], json!({"type":"rectangle","x":100,"y":80,"width":200,"height":100}));
        let created: Value = serde_json::from_str(result.first_text()).unwrap();
        assert_eq!(traced.activity[1]["phase"], "done");
        assert_eq!(traced.activity[1]["ids"], json!([created["id"]]));
    }
}
