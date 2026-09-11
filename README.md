# Agent Plugins

Multi-harness agent plugins. One marketplace for Claude Code and Codex.

```sh
claude plugin marketplace add kjanat/agent-plugins
codex plugin marketplace add kjanat/agent-plugins
```

| Plugin                              | Claude Code | Codex | Description                                       |
| ----------------------------------- | ----------- | ----- | ------------------------------------------------- |
| [typescript](plugins/typescript/)   | yes         | no    | TypeScript 7 native language server (`tsc --lsp`) |
| [blender-mcp](plugins/blender-mcp/) | yes         | yes   | Blender Lab's official Blender MCP server         |

## Layout

```
.claude-plugin/marketplace.json           Claude Code catalog
.agents/plugins/marketplace.json          Codex catalog
plugins/<name>/.claude-plugin/plugin.json Claude Code manifest
plugins/<name>/.codex-plugin/plugin.json  Codex manifest
plugins/<name>/                           shared skills/, .mcp.json, assets/
```
