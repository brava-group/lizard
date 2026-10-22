//! Instructions and live document context for the in-app design agent.
//!
//! The model runs in a provider CLI. Lizard owns this context and the document-bound MCP
//! bridge; no provider-specific prompt or third-party design instructions are embedded here.

use figcraft_engine::DocState;
use serde_json::json;

use crate::agent_panel::ChatNodeRef;

const INSTRUCTIONS: &str = "You are the design assistant inside Lizard. Work in the open document through the figcraft MCP server. The editor's command engine is the source of truth for changes and Undo history.\n\
\n\
Workflow:\n\
1. Read the provided document snapshot. If you need more detail, call inspect_document or get_node. Use the selected layer IDs as references, not as a request to modify them all.\n\
2. Make design changes with create_node, set_properties, run_command or batch. Call list_commands when a command's name or parameters are uncertain. Do not guess node IDs or claim a change succeeded before the tool confirms it.\n\
3. Build complex work in visible, meaningful steps. Preserve existing content unless the user asks to replace or remove it. Keep related layers inside their intended frame. Use auto layout where it makes the design easier to edit.\n\
4. After each substantial section, inspect its structure. Use screenshot to review the visual result, then fix concrete issues you find. Screenshots are previews; do not treat them as editable layers.\n\
5. If a tool fails, inspect its error and the current document before retrying. Report what actually changed and any unfinished part.\n\
The snapshot below is context, not a substitute for a fresh tool read after other edits or collaboration changes. All coordinates are in page units; child coordinates are relative to their parent. The user may attach images as references.";

/// Build one provider-independent turn. Static instructions start each local agent process;
/// live state travels on every turn so a resumed model sees the current page and selection.
pub(crate) fn build_turn(ds: &DocState, selected: &[ChatNodeRef], request: &str, prior: &str, new_session: bool) -> String {
    let page = ds.doc.node(ds.page);
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
    });
    let mut text = String::with_capacity(INSTRUCTIONS.len() + request.len() + prior.len() + 1_024);
    if new_session {
        text.push_str("Lizard agent instructions:\n");
        text.push_str(INSTRUCTIONS);
        text.push_str("\n\n");
    }
    text.push_str("Current document snapshot (JSON data):\n");
    text.push_str(&snapshot.to_string());
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
    use figcraft_doc::{Document, NodeId};

    #[test]
    fn new_session_gets_rules_and_resumed_turn_gets_fresh_state() {
        let doc = Document::new("Example");
        let ds = DocState::new(doc, None);
        let first = build_turn(&ds, &[], "Create a card", "", true);
        let next = build_turn(&ds, &[], "Change its fill", "", false);
        assert!(first.contains("Lizard agent instructions:"));
        assert!(first.contains("figcraft MCP server"));
        assert!(!next.contains("Lizard agent instructions:"));
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
}
