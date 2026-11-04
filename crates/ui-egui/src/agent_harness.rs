//! Instructions and live document context for the in-app design agent.
//!
//! The model runs in a provider CLI. Lizard owns this context and the document-bound MCP
//! bridge; no provider-specific prompt or third-party design instructions are embedded here.

use lizard_engine::DocState;
use serde_json::json;

use crate::agent_panel::ChatNodeRef;

const INSTRUCTIONS: &str = "You are the design assistant inside Lizard. Work in the open document through the lizard MCP server. The editor's command engine is the source of truth for changes and Undo history.\n\
\n\
Workflow:\n\
1. Read the provided document snapshot. If you need more detail, call inspect_document or get_node. Use the selected layer IDs as references, not as a request to modify them all.\n\
2. Make design changes with create_node, set_properties, run_command or batch. Call list_commands when a command's name or parameters are uncertain. Do not guess node IDs or claim a change succeeded before the tool confirms it.\n\
3. Build complex work in visible, meaningful steps. Create each visible section through separate create_node calls so the user can watch it appear on the canvas; avoid one large batch for an entire screen. Use batch for small atomic edits that must succeed or fail together. Preserve existing content unless the user asks to replace or remove it. Keep related layers inside their intended frame. Use auto layout where it makes the design easier to edit.\n\
4. After each substantial section, inspect its structure. Use screenshot to review the visual result, then fix concrete issues you find. Screenshots are previews; do not treat them as editable layers.\n\
5. If a tool fails, inspect its error and the current document before retrying. Report what actually changed and any unfinished part.\n\
For interface icons, use search_icons before creating a layer with type icon. Reuse the snapshot's preferred_icon_library for all icons on the same screen. Search another library only if that family has no suitable icon, and use the returned exact library/name pair. Edit an existing icon with icon.set through run_command. Prefer catalog icons to hand-drawn paths or generated SVGs for ordinary UI glyphs. Follow an explicit library choice from the user.\n\
The snapshot below is context, not a substitute for a fresh tool read after other edits or collaboration changes. All coordinates are in page units; child coordinates are relative to their parent. The user may attach images as references.";

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
        "page_id": ds.page.to_string(),
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
    text.push_str("\nFor UI icons, call search_icons with preferredLibrary from this snapshot; keep that family unless it has no suitable match. Create the returned exact icon via create_node type icon, with library, icon and color.");
    text.push_str(
        "\nBuild visible sections with separate create_node calls so they appear progressively on the canvas. Reserve batch for small atomic edits.",
    );
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
        assert!(first.contains("search_icons"));
        assert!(first.contains("\"preferred_icon_library\":\"lucide\""));
        assert!(!next.contains("Lizard agent instructions:"));
        assert!(next.contains("appear progressively on the canvas"));
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
}
