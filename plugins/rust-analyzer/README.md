# rust-analyzer

Rust code intelligence for Claude Code on **64-bit Windows 10 or newer**, with a memory budget
covering rust-analyzer and its compiler/proc-macro children. Claude stays outside that budget.

## Install

Requires Windows PowerShell 5.1/.NET Framework, Claude Code with LSP `userConfig` support, and
`rust-analyzer.exe` on `PATH`. The rustup proxy selects the workspace's Rust toolchain:

```powershell
rustup component add rust-analyzer rust-src
claude plugin marketplace add kjanat/agent-plugins
claude plugin install rust-analyzer@kjanat --scope local
claude plugin disable rust-analyzer-lsp@claude-plugins-official --scope local
```

Run the plugin commands from the project where you want protection. Disable any other Rust LSP
plugin there, including `rust-guard@local-rust-guard` if you used the earlier standalone setup;
multiple plugins claiming `.rs` can select the wrong server. Restart Claude after switching.

For an unpublished local checkout, replace the marketplace-add source above with its absolute
directory path. To try the plugin without registering a marketplace, start Claude with
`--plugin-dir /absolute/path/to/agent-plugins/plugins/rust-analyzer` after disabling competing
Rust plugins in that project.

There are no downloaded guard binaries or installation build steps. On each LSP launch, Windows
PowerShell compiles the bundled C# source into its own process, then starts rust-analyzer with
native inherited pipes. Compilation adds startup overhead; no persistent executable cache is
maintained. A missing server, invalid budget, unsupported host, or failed job setup refuses the
launch rather than falling back to an unbounded server. There is no macOS/Linux launcher.

## Configuration

Use the plugin's configuration in `/config`, or provide values at installation:

```powershell
claude plugin install rust-analyzer@kjanat --scope local --config memory_limit_mib=4096
```

| Option             | Default             | Meaning                                               |
| ------------------ | ------------------- | ----------------------------------------------------- |
| `memory_limit_mib` | `6144`              | Combined committed-memory ceiling, in whole MiB       |
| `server`           | `rust-analyzer.exe` | Executable on PATH, or an absolute native `.exe` path |

The launcher accepts budgets from 32 to 1,048,576 MiB; fractional values are rejected. The lower
end exists for bounded containment tests, not practical Rust analysis. An explicit server path
can pin a binary instead of using rustup's workspace selection. Batch/script wrappers and relative
paths are rejected.

The LSP settings in `.lsp.json` keep navigation, diagnostics, proc macros and build scripts enabled,
while using two analysis workers, at most two Cargo jobs, and no eager cache priming. These reduce
background work; they are separate from the enforced memory ceiling. Thread settings may make
analysis slower, and disabled priming defers work until requested.

## Failure behavior

The launcher assigns the server to a Windows Job Object **atomically at process creation**. Its
descendants inherit membership and cannot opt out of the job. The non-inherited job handle stays
with the launcher.

- Windows rejects allocations that would exceed the combined committed-memory ceiling.
- Every 100 ms, the launcher checks the job's high-water mark. At 95% of the ceiling it terminates
  the entire LSP group with exit code 137 and explains the budget stop on stderr.
- The hard ceiling remains effective between polls. A fast failed allocation may make the server
  exit on its own first; in that case its exit code is preserved.
- Normal server exit, launcher errors, or killing the launcher also clean up remaining children.
- LSP crash restarts are disabled. Budget exhaustion leaves Rust intelligence unavailable until
  restarted; it does not repeatedly relaunch the same workload.

The default 6 GiB ceiling is a starting budget, not a claim that every workspace fits. It caps one
LSP tree, not all Claude sessions together. The PowerShell supervisor is outside the budget.
This neither reserves system headroom nor prevents unrelated applications from exhausting memory.
It also does not alter Bun crash handling or restore terminal modes after unrelated Claude crashes.

## Development and tests

Source lives in `native/Guard.cs`; `scripts/start.ps1` resolves the server and invokes it in-process.
Do not launch the server through a PowerShell text pipeline: LSP requires byte-preserving stdio.

`native/RustAnalyzerGuard.csproj` gives editors the actual .NET Framework 4.8/C# 5 target.
`DllImport` is intentional: Windows PowerShell's `Add-Type` does not provide .NET 7's
`LibraryImport` source generator. The project is for development checks; startup still compiles
the source directly and requires no .NET SDK.

```powershell
claude plugin validate --strict plugins/rust-analyzer
python plugins/rust-analyzer/tests/test_guard.py
python plugins/rust-analyzer/tests/test_guard.py --lsp
```

The tests require Python 3.10+ and the Windows .NET Framework C# compiler. They compile a temporary
helper, exercise a 96 MiB budget, and remove their temporary files. They cover binary stdio and
stderr separation, exit codes, path/argument quoting, child memory accounting, whole-group
termination, cleanup after supervisor death and normal server exit, and invalid configurations.
`--lsp` also initializes and shuts down the installed rust-analyzer on an empty workspace. It does
not index a large repository or reproduce full-system memory exhaustion.

Validated on Windows x64 with Claude Code 2.1.289 and rust-analyzer 1.98.1. Full Zed indexing under
the chosen budget has not been tested.

## References

- [Windows job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information)
- [Atomic job assignment](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812)
- [Claude LSP configuration](https://code.claude.com/docs/en/plugins-reference#lspservers)

<!-- markdownlint-disable-file -->
