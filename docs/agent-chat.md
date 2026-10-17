# Agent chat

The **Agent** button in the left rail opens a chat for the current document. The desktop app
starts a local [Agent Client Protocol (ACP)](https://agentclientprotocol.com/) adapter and gives
it FigCraft's existing MCP server, attached to that document. Design changes still go through
the normal engine commands, appear on the canvas and enter Undo history.

The menu in the chat header chooses **Codex**, **Claude Code** or **Gemini CLI**. Install the
corresponding ACP executable and make it available on `PATH`:

| Choice | Executable started by FigCraft |
|---|---|
| Codex | `codex-acp` |
| Claude Code | `claude-agent-acp` |
| Gemini CLI | `gemini --acp` |

The adapter handles its own sign-in and model connection. FigCraft itself does not store model
credentials. It starts `figcraft-cli mcp --attach <private-port>` for the agent; installed builds
should keep `figcraft-cli` beside `figcraft`, or make it available on `PATH`. The private MCP
bridge checks the active document before accepting an operation and is revoked when the chat
session changes. ACP tool approvals appear in the chat before the adapter continues.

Replies stream into the panel with their tool calls. **Stop** cancels the current session. The
history is kept locally in `agent-chats.json` in FigCraft's user configuration directory (owner
read/write permissions on Unix), grouped by document. Opening an older chat starts a fresh ACP
session and supplies its recent transcript as context. A completed answer offers **Undo this
answer's changes** only when every Undo step it added came from the agent and no later edit has
changed that history. Shift+Enter inserts a newline; Enter sends.

The browser build retains the chat layout but cannot start local ACP executables. Specialized
image generation, background removal and visual search are separate AI features and remain open.
