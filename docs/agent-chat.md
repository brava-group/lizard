# Agent chat

The **Agent** button in the left rail opens a chat for the current document. The desktop app
starts the selected local agent and gives it FigCraft's existing MCP server, attached to that
document. Design changes still go through
the normal engine commands, appear on the canvas and enter Undo history.

The in-app harness prepares each turn before it reaches the selected provider. A new local
agent process receives FigCraft's own editing instructions: use the attached MCP tools, make
changes through commands, verify structure and screenshots, and report only confirmed work. Every turn
also gets a bounded JSON snapshot of the current document, page, revision, selected layers and
top-level layers. The agent can request more detail through `inspect_document` and `get_node`;
the snapshot is not assumed to stay current after later edits. A running local session keeps its
original instructions, so FigCraft sends only fresh context and the user's next request. The
first turn after reopening a chat repeats the instructions in case its ACP adapter cannot restore
the prior remote session. Recent transcript text is supplied to ACP after a local restart;
Codex relies on its saved thread instead.

The menu in the chat header chooses **Codex**, **Claude Code**, **Gemini CLI** or **Kimi Code**.
Codex uses the installed Codex CLI's [app-server](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
and its ChatGPT subscription login. The Agent welcome screen shows the connection state;
**Conectar com ChatGPT** starts browser sign-in when needed. The model selector in the composer
footer lists models returned by Codex, plus its default model. The adjacent lightning control
selects one to six agents, and the grid control selects one to six variants. The composer uses
24 px footer controls with brief hover fades; the model field uses the property-panel field
colours and hugs the model name while leaving free space before Send. The model list uses the
property-panel popup component. It opens above the field and closes on an
outside click or Escape.
Reduced-motion preferences make those hover changes immediate. Multiple agents use
separate local sessions and document-scoped MCP bridges; their streamed messages and screenshot
previews appear under separate agent labels. Side-by-side mode asks each agent to create an
independent root frame, while split mode assigns each a distinct part of the same request. Variants
run in successive rounds after all agents in the previous round finish. The variants control can
request changes in layout or visual style. Parallel edits still share one canvas, so the agents'
instructions coordinate their work without hard layer ownership. FigCraft stores neither tokens
nor API keys. Install the
corresponding executable:

| Choice | Executable started by FigCraft |
|---|---|
| Codex | `codex app-server --stdio` |
| Claude Code | `claude-agent-acp` |
| Gemini CLI | `gemini --acp` |
| Kimi Code | `kimi acp` |

The other agents use [ACP](https://agentclientprotocol.com/) and their own CLI account
configuration. The panel links to each provider's installation instructions when its executable
is missing. FigCraft starts `figcraft-cli mcp --attach <private-port>` for the agent; installed builds
should keep `figcraft-cli` beside `figcraft`, or make it available on `PATH`. The private MCP
bridge checks the active document before accepting an operation and is revoked when the chat
session changes. ACP tool approvals appear in the chat before the adapter continues.

Replies stream into the panel with their tool calls. **Stop** cancels the current session. The
history is kept locally in `agent-chats.json` in FigCraft's user configuration directory (owner
read/write permissions on Unix), grouped by document. Each Codex worker resumes by its saved thread ID;
ACP workers resume a saved session when their `loadSession` capability is available. Recent transcript
text remains available as context for ACP agents. A completed answer offers **Undo this
answer's changes** only when every Undo step it added came from the agent and no later edit has
changed that history. Shift+Enter inserts a newline; Enter sends.

Selected canvas layers appear above the prompt as references. A chip locates its layer on the
canvas; its × removes that layer from the next message's context without deselecting or deleting
it. The conversation records the names and IDs that were actually sent. The paperclip button accepts
PNG/JPEG images from a file or the clipboard, and pasting an image while the prompt has focus
adds an attachment even when the clipboard also reports alternate text. Images appear as removable,
rounded 72 px square thumbnails above the prompt; selected layer chips and sent attachment badges
use the panel theme colours. Images are sent as separate inputs to Codex or as ACP image blocks
when the chosen adapter advertises image prompt support. Up to four images, totalling 4 MB, can be attached to one
message; failed sends restore them to the composer. Text, HTML and
copied design layers continue through their existing paste paths. Agent activity is grouped under
an expandable work receipt, while the answer and guarded Undo remain visible.

The conversation follows the current dark panel reference: user messages are right aligned in
rounded bubbles; each answer shows the agent, model and variant, thought time, tool receipts,
preview images and final Markdown response. While a turn runs, the receipt shows a loading spinner, agent reasoning summaries when the
adapter supplies them, and each tool's progress. MCP screenshots used for design review appear
as bounded image previews in the reply. These previews are temporary and are not added to the
design file or chat history. Agent replies render CommonMark headings, emphasis, lists, links and
code in the chat; raw HTML is displayed as text, and the prompt field stays plain text.
Attached MCP calls pass the IDs of inspected or changed layers to a
document-scoped UI overlay: the agent cursor moves toward the current target, inspection briefly
highlights visible frames, and edits pulse on the affected layer. An active creation inside a
frame gives that frame a subtle animated outline. The overlay never enters Undo, export or the
document; reduced-motion settings remove the moving scan and cursor glide.

The browser build retains the chat layout but cannot start local agent executables. File attachment
dialogs use the desktop file picker. Specialized image generation, background removal and visual
search are separate AI features and remain open.
