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
3. Plan a semantic frame hierarchy before placing details. For interface screens, use auto layout by default: choose the screen direction from the design (horizontal for desktop columns, vertical for mobile stacks), nest horizontal rows for navigation and controls, vertical stacks for cards and feeds, and frames for reusable components. Set layout, gap, padding, mainAlign, crossAlign and sizingH/sizingV (hug or fill) on the parent and children. Children in auto layout follow the flow; do not position them with x/y or leave related UI elements loose on the page. Use absolute positioning only for intentional overlays or decoration. When extending an existing screen, inspect its parent layout and preserve or improve that structure.\n\
4. Build in visible, meaningful sections with batch. For a new screen, create its root frame, then use about 2-4 batch calls of roughly 10-30 node.create steps; the editor streams every step to the canvas, so a larger batch still shows progress. Create section frames and their children in the same batch. Give nested frames an `as` name and refer to their ids with `$ref` later in that batch. Set layout, sizing, text and paint in each node.create's props instead of following it with set_properties. Do not call create_node separately for every section, text, icon or small shape. Preserve existing content unless the user asks to replace or remove it.\n\
5. Inspect the assembled screen once with inspect_document or get_node to verify auto layout and hug/fill sizing. Take one screenshot, fix concrete visual issues, and capture again only when the fix needs visual verification. Inspect earlier only when a batch fails or existing structure is uncertain. Screenshots are previews; do not treat them as editable layers.\n\
6. If a tool fails, inspect its error and the current document before retrying. Report what actually changed and any unfinished part.\n\
For interface icons, gather the names needed by a section and call search_icons once with queries (up to 12 names) before creating icon layers. Reuse returned exact library/name pairs; do not search the same name repeatedly. Reuse the snapshot's preferred_icon_library for all icons on the same screen. Search another library only if that family has no suitable icon. Edit an existing icon with icon.set through run_command. Prefer catalog icons to hand-drawn paths or generated SVGs for ordinary UI glyphs. Follow an explicit library choice from the user.\n\
The snapshot below is context, not a substitute for a fresh tool read after other edits or collaboration changes. All coordinates are in page units; child coordinates are relative to their parent. The user may attach images as references.";

const COMPONENT_WORKFLOW: &str = "\nReusable component workflow: A frame is not yet a component. Build the full cross-product of requested properties: two styles and three sizes require six distinct variants. Make each one an auto-layout frame with real text, fills, padding and sizing. Name variants with property pairs, for example `Button/Style=Ghost, Size=sm`, `Button/Style=Secondary, Size=sm`, and so on for the complete requested combinations. Use the returned node IDs, then call run_command with `component.create` and `params:{\"ids\":[id]}` for each frame. After those calls succeed, call run_command with `component.combineVariants` and `params:{\"ids\":[component IDs]}` to make one component set. For a single reusable component, stop after component.create. Do not use ordinary groups or frames as a substitute for main components. Inspect the returned set ID with get_node or inspect_document: it must be a component set with the requested number of component children. Then run `component.properties` on that set ID and confirm the requested property names, values and no conflicts. Check the canvas before finishing. If any command fails or the structure is incomplete, say exactly what exists and what failed; never claim that the component or variants were created without that verification.";

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
        text.push_str("\n\n");
    }
    text.push_str("Current document snapshot (JSON data):\n");
    text.push_str(&snapshot.to_string());
    text.push_str("\nUse only the lizard MCP server. Before changing this document, inspect_document must return documentId matching snapshot.document_id and currentPage matching snapshot.page_id; otherwise stop and report the mismatch.");
    text.push_str("\nFor UI icons, search several needed names in one search_icons call using queries and preferredLibrary from this snapshot. Reuse the returned exact library/icon names and keep that family unless it has no suitable match.");
    text.push_str("\nBuild progressively with streamed batches: create the screen root, then create section frames and children in about 2-4 batches of 10-30 node.create steps using props at creation and `$ref` for nested parents. Inspect the assembled screen once and take one screenshot; inspect earlier only after a tool error or when modifying uncertain existing structure.");
    text.push_str("\nFor interface screens, build a nested auto-layout frame hierarchy (screen, sections, rows, cards, lists). Use horizontal layout for desktop columns and vertical layout for mobile stacks; set gap, padding and sizingH/sizingV hug/fill. Keep children in flow instead of placing them by x/y. Use absolute only for intentional overlays or decoration. Verify the resulting hierarchy before finishing.");
    let request_lower = request.to_lowercase();
    if ["component", "componente", "variant", "variante", "reutiliz"].iter().any(|word| request_lower.contains(word)) {
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
        assert!(next.contains("Build progressively with streamed batches"));
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
        assert!(turn.contains("component.combineVariants"));
        assert!(turn.contains("component set with the requested number of component children"));
        assert!(turn.contains("never claim that the component or variants were created without that verification"));
    }
}
