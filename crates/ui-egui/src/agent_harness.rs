//! Instructions and live document context for the in-app design agent.
//!
//! The model runs in a provider CLI. Lizard owns this context and the document-bound MCP
//! bridge; no provider-specific prompt or third-party design instructions are embedded here.

use lizard_engine::DocState;
use serde_json::json;

use crate::agent_panel::ChatNodeRef;

const INSTRUCTIONS: &str = "You are the design assistant inside Lizard. Work in the open document through the lizard MCP server only. Do not use another MCP server even if it offers the same tool names. The editor's command engine is the source of truth for changes and Undo history.\n\
\n\
Workflow:\n\
1. Call inspect_document on the lizard MCP server and compare its `documentId` and `currentPage` with `document_id` and `page_id` in the provided snapshot before editing. If the IDs are absent or differ, stop and report that the tool is connected to another document. Use the selected layer IDs as references, not as a request to modify them all.\n\
2. Make design changes with create_node, set_properties, run_command or batch. Call list_commands when a command's name or parameters are uncertain. Do not guess node IDs or claim a change succeeded before the tool confirms it.\n\
3. Plan a semantic frame hierarchy before placing details. For interface screens, use auto layout by default: choose the screen direction from the design (horizontal for desktop columns, vertical for mobile stacks), nest horizontal rows for navigation and controls, vertical stacks for cards and feeds, and frames for reusable components. Set layout, gap, padding, mainAlign, crossAlign and sizingH/sizingV (hug or fill) on the parent and children. Nested frames used only for structure must be transparent. Give an explicit fill only to intended visible surfaces such as the screen background, cards, buttons, and panels. Children in auto layout follow the flow; do not position them with x/y or leave related UI elements loose on the page. Use absolute positioning only for intentional overlays or decoration. When extending an existing screen, inspect its parent layout and preserve or improve that structure.\n\
4. Build in visible, meaningful sections with batch. For a new screen, create its root frame, then use about 2-4 batch calls of roughly 10-30 node.create steps; the editor streams every step to the canvas, so a larger batch still shows progress. Create section frames and their children in the same batch. Give nested frames an `as` name and refer to their ids with `$ref` later in that batch. Set layout, sizing, text and paint in each node.create's props instead of following it with set_properties. Do not call create_node separately for every section, text, icon or small shape. Preserve existing content unless the user asks to replace or remove it.\n\
5. Keep building the next section while the read-only orchestrator reviews completed sections. Batch or inspect results may include an orchestrator review as an extra text block. Check its cited node IDs; fix verified issues at the next safe batch boundary without restarting the screen. Do not pause waiting for the review. Inspect the assembled screen once with inspect_document or get_node to verify auto layout and hug/fill sizing. Take one screenshot, fix concrete visual issues, and capture again only when the fix needs visual verification. Inspect earlier only when a batch fails or existing structure is uncertain. Screenshots are previews; do not treat them as editable layers.\n\
6. If a tool fails, inspect its error and the current document before retrying. Report what actually changed and any unfinished part.\n\
For interface icons, gather the names needed by a section and call search_icons once with queries (up to 12 names) before creating icon layers. Reuse returned exact library/name pairs; do not search the same name repeatedly. Reuse the snapshot's preferred_icon_library for all icons on the same screen. Search another library only if that family has no suitable icon. Edit an existing icon with icon.set through run_command. Prefer catalog icons to hand-drawn paths or generated SVGs for ordinary UI glyphs. Follow an explicit library choice from the user.\n\
The snapshot below is context, not a substitute for a fresh tool read after other edits or collaboration changes. All coordinates are in page units; child coordinates are relative to their parent. The user may attach images as references.";

const COMPONENT_PRINCIPLES: &str = r#"
Design-system semantics: Before making reusable UI, classify each difference. A primitive is an independently reusable building block (for example an icon glyph); a composed component combines primitives (for example a Quick Action Button or Navigation Item Button). A text label is a text property, an interchangeable icon is an instance-swap property backed by nested icon component instances, and optional visibility is a boolean property. For size, style and state differences that change field values on the same layer structure, use one main component with local matrix axes and cells. A matrix axis is local to its component; document variables are shared tokens. Use a separate drawn variant only when the layer structure or artwork genuinely changes. Do not make a variant for each label, icon, action, destination, transaction, or sample content. Do not draw the cross-product of size, style and state. Keep one canonical component per reusable role and place configured instances in screens. Create only states the request or design actually needs; do not invent hover/pressed screens merely to fill a catalog. If the request is to componentize an existing screen, reuse its visual language, build real native components, replace repeated copies with instances, and verify the screen still matches. State clearly if the result is only a visual sample rather than a reusable component."#;

const COMPONENT_WORKFLOW: &str = r#"
Reusable component workflow:
1. Inventory repeated roles and classify each changing field as primitive, text property, instance-swap property, boolean property, variant, or shared token. Explain ambiguous choices briefly in the result. For a banking screen, `Send`, `Request`, and `Add money` are values of Label and Icon on one Quick Action Button, not three variants; Home, Accounts, Cards, and Profile are values on one Navigation Item Button. Selection may be a variant because its appearance changes.
2. Build the required icon primitives first as actual main components, each with artwork in a consistent-size frame. Reuse existing local or enabled-library icon components when available. Build each composed control with auto layout, one text layer, and a nested instance of a default icon component. Convert its frame using run_command `component.create` with `params:{"ids":[frameId]}` and use the returned component ID.
3. Define editable properties with `component.addProperty`: `{"id":componentId,"name":"Label","type":"text","default":"Send","layer":textLayerId}` and `{"id":componentId,"name":"Icon","type":"instanceSwap","default":defaultIconComponentId,"layer":nestedIconInstanceId,"preferred":[iconComponentIds]}`. Use `component.bindProperty` with field `characters` or `mainComponent` if a property was added without a layer binding. Use a boolean property for optional parts. A plain icon layer or a renamed frame is not an instance-swap property.
4. If sizes, styles or states change values on the same layer structure, use `component.matrixSetAxis` on the main (for example Size with Small, Medium, Large). Bind fields with `component.matrixSetCell`: `{"id":mainId,"layer":layerId,"field":"padding","when":{"Size":"Small"},"value":[6,10,6,10]}`. One field may depend on two axes, for example fill with `when:{"Style":"Primary","State":"Hover"}`. Add only necessary cells and use `when:{}` as a fallback. Use `component.matrixDescribe` to inspect resolved values. Use drawn native variants and `component.combineVariants` only for structural or artwork differences. Do not combine unrelated control roles in one set.
5. Put `instance.create` results in the screen, then set their content with `instance.setProperties`, for example `{"ids":[instanceId],"properties":{"Label":"Request","Icon":requestIconComponentId}}`. Set variant values only for applicable states. Do not leave duplicate detached frames in place of the screen's reusable controls.
6. Verify with `component.properties` and `component.matrixDescribe` that the axes, cells and resolved values match the design. Inspect at least two instances with different axis selections to confirm their main-component links and rendered values. Check a screenshot for visual integrity. Report only components, properties, matrices and instances confirmed by tool results; if any step fails, say what exists and what failed."#;

/// Build one provider-independent turn. Static instructions start each local agent process;
/// live state travels on every turn so a resumed model sees the current page and selection.
pub(crate) fn build_turn(ds: &DocState, selected: &[ChatNodeRef], request: &str, prior: &str, new_session: bool) -> String {
    let page = ds.doc.node(ds.page);
    let mut icon_scope = selected.first().map_or(ds.page, |reference| reference.id);
    for _ in 0..64 {
        let Some(node) = ds.doc.node(icon_scope) else {
            icon_scope = ds.page;
            break;
        };
        if node.kind.is_frame_like() || icon_scope == ds.page {
            break;
        }
        icon_scope = node.parent.unwrap_or(ds.page);
    }
    let preferred_icon_library = lizard_engine::cmd::icon::dominant_library(&ds.doc, icon_scope, 2_048)
        .or_else(|| lizard_engine::cmd::icon::dominant_library(&ds.doc, ds.page, 2_048))
        .unwrap_or("lucide");
    let selected = selected
        .iter()
        .take(32)
        .filter_map(|reference| {
            ds.doc.node(reference.id).map(|node| {
                json!({
                    "id": node.id.to_string(),
                    "name": node.name.chars().take(160).collect::<String>(),
                    "type": node.kind.type_name(),
                    "width": node.width,
                    "height": node.height,
                })
            })
        })
        .collect::<Vec<_>>();
    let top_level = page
        .map(|page| {
            page.children
                .iter()
                .take(24)
                .filter_map(|id| ds.doc.node(*id))
                .map(
                    |node| json!({"id": node.id.to_string(), "name": node.name.chars().take(160).collect::<String>(), "type": node.kind.type_name()}),
                )
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let snapshot = json!({
        "document_id": ds.uid,
        "document_name": ds.doc.name.chars().take(160).collect::<String>(),
        "page_id": ds.page,
        "page_name": page.map(|node| node.name.chars().take(160).collect::<String>()),
        "revision": ds.revision,
        "selected_layers": selected,
        "top_level_layers": top_level,
        "top_level_total": page.map_or(0, |node| node.children.len()),
        "preferred_icon_library": preferred_icon_library,
    });
    let mut text = String::with_capacity(INSTRUCTIONS.len() + request.len() + prior.len() + 1_024);
    if new_session {
        text.push_str("Lizard agent instructions:\n");
        text.push_str(INSTRUCTIONS);
        text.push_str(COMPONENT_PRINCIPLES);
        text.push_str("\n\n");
    } else {
        text.push_str(COMPONENT_PRINCIPLES);
        text.push('\n');
    }
    text.push_str("Current document snapshot (JSON data):\n");
    text.push_str(&snapshot.to_string());
    text.push_str("\nUse only the lizard MCP server. Before changing this document, inspect_document must return documentId matching snapshot.document_id and currentPage matching snapshot.page_id; otherwise stop and report the mismatch.");
    text.push_str("\nFor UI icons, search several needed names in one search_icons call using queries and preferredLibrary from this snapshot. Reuse the returned exact library/icon names and keep that family unless it has no suitable match.");
    text.push_str("\nWhen creating a new screen, build progressively with streamed batches: create the screen root, then create section frames and children in about 2-4 batches of 10-30 node.create steps using props at creation and `$ref` for nested parents. When reviewing or editing an existing screen, inspect its hierarchy first and change only the nodes needed. Inspect the assembled screen once and take one screenshot; inspect earlier only after a tool error or when modifying uncertain existing structure.");
    text.push_str("\nFor interface screens, build a nested auto-layout frame hierarchy (screen, sections, rows, cards, lists). Use horizontal layout for desktop columns and vertical layout for mobile stacks; set gap, padding and sizingH/sizingV hug/fill. Keep children in flow instead of placing them by x/y. Use absolute only for intentional overlays or decoration. Verify the resulting hierarchy before finishing.");
    let request_lower = request.to_lowercase();
    if ["component", "componente", "componentiz", "variant", "variante", "reutiliz", "design system", "biblioteca"]
        .iter()
        .any(|word| request_lower.contains(word))
    {
        text.push_str(COMPONENT_WORKFLOW);
    }
    if !prior.is_empty() {
        text.push_str("\n\nRecent conversation (reference only):\n");
        text.push_str(&json!(prior).to_string());
    }
    text.push_str("\n\nUser request:\n");
    text.push_str(request);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use lizard_doc::{Document, NodeId};
    use lizard_engine::Session;

    #[test]
    fn new_session_gets_rules_and_resumed_turn_gets_fresh_state() {
        let doc = Document::new("Example");
        let ds = DocState::new(doc, None);
        let first = build_turn(&ds, &[], "Create a card", "", true);
        let next = build_turn(&ds, &[], "Change its fill", "", false);
        assert!(first.contains("Lizard agent instructions:"));
        assert!(first.contains("lizard MCP server"));
        assert!(first.contains("compare its `documentId`"));
        assert!(first.contains("search_icons"));
        assert!(first.contains("queries (up to 12 names)"));
        assert!(first.contains("batch calls of roughly 10-30 node.create steps"));
        assert!(first.contains("use auto layout by default"));
        assert!(first.contains("horizontal for desktop columns"));
        assert!(first.contains("\"preferred_icon_library\":\"lucide\""));
        assert!(!next.contains("Lizard agent instructions:"));
        assert!(next.contains("build progressively with streamed batches"));
        assert!(next.contains("documentId matching snapshot.document_id"));
        assert!(next.contains("using props at creation and `$ref`"));
        assert!(next.contains("nested auto-layout frame hierarchy"));
        assert!(next.contains("horizontal layout for desktop columns"));
        assert!(next.contains("\"document_name\":\"Example\""));
        assert!(next.contains("Change its fill"));
    }

    #[test]
    fn selection_is_data_and_stale_ids_are_omitted() {
        let doc = Document::new("Example");
        let ds = DocState::new(doc, None);
        let selected =
            [ChatNodeRef { id: ds.page, name: "ignored supplied name".into() }, ChatNodeRef { id: NodeId(u64::MAX), name: "stale".into() }];
        let turn = build_turn(&ds, &selected, "Inspect", "assistant: done", false);
        assert!(turn.contains("\"type\":\"PAGE\""));
        assert!(!turn.contains("ignored supplied name"));
        assert!(!turn.contains("stale"));
        assert!(turn.contains("Recent conversation (reference only):"));
    }

    #[test]
    fn selected_screen_keeps_its_existing_icon_library() -> lizard_engine::Result<()> {
        let mut session = Session::new();
        let frame = session.execute("node.create", &json!({"type":"frame", "name":"Settings screen", "width":400, "height":400}))?;
        let id = lizard_engine::params::id(&frame, "id").ok_or_else(|| lizard_engine::bad("test", "missing frame id"))?;
        session.execute("icon.create", &json!({"parent":id, "library":"phosphor", "icon":"gear", "select":false}))?;
        let selected = [ChatNodeRef { id, name: "Settings screen".into() }];
        let turn = build_turn(session.ds()?, &selected, "Add a save icon", "", false);
        assert!(turn.contains("\"preferred_icon_library\":\"phosphor\""));
        Ok(())
    }

    #[test]
    fn reusable_button_request_gets_component_creation_and_verification_steps() {
        let ds = DocState::new(Document::new("Example"), None);
        let turn = build_turn(&ds, &[], "Crie um componente de botão com variantes ghost e secondary em sm, md e lg", "", false);
        assert!(turn.contains("component.create"));
        assert!(turn.contains("component.matrixSetAxis"));
        assert!(turn.contains("component.matrixSetCell"));
        assert!(turn.contains("component.matrixDescribe"));
        assert!(turn.contains("instanceSwap"));
        assert!(turn.contains("component.addProperty"));
        assert!(turn.contains("instance.setProperties"));
        assert!(turn.contains("Do not draw the cross-product of size, style and state"));
        assert!(turn.contains("Report only components, properties, matrices and instances confirmed by tool results"));
    }

    #[test]
    fn componentize_screen_request_distinguishes_content_from_variants() {
        let ds = DocState::new(Document::new("Example"), None);
        let turn = build_turn(&ds, &[], "Componentize a banking screen into a design system", "", false);
        assert!(turn.contains("`Send`, `Request`, and `Add money` are values of Label and Icon"));
        assert!(turn.contains("Quick Action Button"));
        assert!(turn.contains("Navigation Item Button"));
        assert!(turn.contains("one canonical component per reusable role"));
        assert!(!turn.contains("Build the full cross-product"));
    }
}
