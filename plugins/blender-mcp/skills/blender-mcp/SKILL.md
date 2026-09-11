---
name: blender-mcp
description: Use Blender Lab's official Blender MCP server to inspect, modify, navigate, document, or render a live Blender scene. Use when the user asks to work with Blender, a .blend file opened in Blender, Blender Python APIs, scene data, objects, materials, or renders.
---

# Blender MCP

Use the `blender` MCP tools for work against the user's running Blender instance.

## Before tool calls

- Confirm Blender is open with the intended file.
- If the Blender tools cannot connect, tell the user to install and enable the MCP extension from the Blender Lab repository at `https://lab.blender.org/`, then retry.
- Treat tool calls that execute Python or change scene data as writes. Inspect first when the user's intent is diagnostic or ambiguous.

## Working safely

- Save or duplicate important `.blend` files before broad or destructive edits.
- Prefer the server's scene-summary and object-detail tools before executing custom Python.
- Use bundled API/manual search tools to verify Blender-specific behavior before writing Python.
- Keep generated files in paths the user approved and report those paths after rendering or exporting.
- Explain any mutation that may be difficult to undo before performing it.

## Good starting points

- Summarize a scene with the object and blend-file summary tools.
- Inspect a specific object before changing transforms, modifiers, materials, or data-blocks.
- Use screenshot and navigation tools when the task depends on the current Blender UI state.
- Use render tools for previews and custom Python only when the higher-level tools do not cover the task.
