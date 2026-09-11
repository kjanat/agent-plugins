# typescript

Runs the TypeScript 7 native language server (`tsc --lsp --stdio`) in Claude Code.

## Install

```sh
claude plugin marketplace add kjanat/agent-plugins
claude plugin install typescript@kjanat
```

The plugin pins `typescript@^7` in its own `package.json`. Claude Code installs it into the plugin
cache with `bun install --frozen-lockfile` when the plugin is installed or updated. Bun must be on
`PATH`. No global TypeScript install is needed.

Disable the official `typescript-lsp` plugin if it is enabled. Both claim the same file extensions
and only the first registered server starts.

## How the server runs

`bin/tsc-lsp` executes the native binary from `@typescript/typescript-<os>-<arch>` when one exists
for the current platform, otherwise it runs the package's `bin/tsc` Node launcher. The server
process always gets `VOLTA_BYPASS=1`.

## Configuration

Edit `.lsp.json`. `initializationOptions` go to the server at `initialize`. `settings` go to the
server via `workspace/didChangeConfiguration`, in the `js/ts`, `typescript`, and `javascript`
sections the server reads. The option names match the VS Code TypeScript settings without the
`typescript.`/`javascript.` prefix.

```json
{
	"typescript": {
		"initializationOptions": { "enableTelemetry": false },
		"settings": {
			"typescript": {
				"preferences": { "quoteStyle": "single", "importModuleSpecifier": "non-relative" }
			}
		}
	}
}
```

## Extensions

`.ts` `.mts` `.cts` `.tsx` `.js` `.mjs` `.cjs` `.jsx`
