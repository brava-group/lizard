# Agent chat

The **Agent** button in the left rail opens a chat for the current document. The desktop app
starts the selected local agent and gives it FigCraft's existing MCP server, attached to that
document. Design changes still go through
the normal engine commands, appear on the canvas and enter Undo history.

The menu in the chat header chooses **Codex**, **Claude Code**, **Gemini CLI** or **Kimi Code**.
Codex uses the installed Codex CLI's [app-server](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
and its ChatGPT subscription login. The Agent welcome screen shows the connection state;
**Conectar com ChatGPT** starts browser sign-in when needed. The IA badge lists models returned
by Codex, plus its default model. FigCraft stores neither tokens nor API keys. Install the
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
read/write permissions on Unix), grouped by document. Codex threads resume by saved ID; ACP
agents resume a saved session when their `loadSession` capability is available. Recent transcript
text remains available as context for ACP agents. A completed answer offers **Undo this
answer's changes** only when every Undo step it added came from the agent and no later edit has
changed that history. Shift+Enter inserts a newline; Enter sends.

Selected canvas layers appear above the prompt as references. A chip locates its layer on the
canvas; its × removes that layer from the next message's context without deselecting or deleting
it. The conversation records the names and IDs that were actually sent. The + button accepts
PNG/JPEG images from a file or the clipboard, and pasting an image while the prompt has focus
adds an attachment even when the clipboard also reports alternate text. Images appear as removable,
rounded 72 px square thumbnails above the prompt; selected layer chips and sent attachment badges
use the panel theme colours. Images are sent as separate inputs to Codex or as ACP image blocks
when the chosen adapter advertises image prompt support. Up to four images, totalling 4 MB, can be attached to one
message; failed sends restore them to the composer. Text, HTML and
copied design layers continue through their existing paste paths. Agent activity is grouped under
an expandable work receipt, while the answer and guarded Undo remain visible.

The browser build retains the chat layout but cannot start local agent executables. File attachment
dialogs use the desktop file picker. Specialized image generation, background removal and visual
search are separate AI features and remain open.
