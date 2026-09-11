# blender-mcp

Runs Blender Lab's official Blender MCP server from the upstream `main` branch:

```sh
uvx -p 3.11 --with 'mcp[cli]<2' --from 'git+https://projects.blender.org/lab/blender_mcp.git@main#subdirectory=mcp' blender-mcp
```

The server talks to the Blender add-on over the local bridge. Works in Claude Code and Codex.

## Install

Claude Code:

```sh
claude plugin marketplace add kjanat/agent-plugins
claude plugin install blender-mcp@kjanat
```

Codex:

```sh
codex plugin marketplace add kjanat/agent-plugins
```

Then install `blender-mcp` from the `kjanat` marketplace in the plugin browser.

## One-time setup

1. Install the Blender Lab Extensions repository from <https://lab.blender.org/>.
2. In Blender, install and enable the MCP extension.
3. Keep Blender open while using the plugin.
4. Start a new session after installing or updating the plugin.

Requires `uv`. `uvx` fetches Python 3.11 when needed. The first launch fetches the repository and
builds an isolated tool environment. The MCP SDK is pinned below 2.x because Blender MCP imports an
API removed in MCP SDK 2.x.

## Security

The server executes Python inside Blender with the same access as your Blender process. Save
important work first and use it only with files and prompts you trust.

## Upstream

- Project: <https://projects.blender.org/lab/blender_mcp>
- Documentation: <https://www.blender.org/lab/mcp-server/>
- License: GPL-3.0-or-later
