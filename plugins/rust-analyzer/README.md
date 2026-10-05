# rust-analyzer

Rust code intelligence for Claude Code on **Windows, Linux and macOS**, with a compiled
.NET 10 supervisor. Claude stays outside the guarded LSP group.

| Platform      | Protection                                                           |
| ------------- | -------------------------------------------------------------------- |
| Windows 10+   | Kernel-enforced committed-memory ceiling for the entire process tree |
| Linux / macOS | Soft watchdog over process-group resident memory (RSS)               |

The Unix watchdog is **not a hard limit**. Fast allocations can exceed the budget before the
next check. It does not use Linux cgroups or macOS virtual-address-space limits.

## Install

Requires a **64-bit .NET 10 SDK**, Python only for tests, Claude Code with plugin Setup hooks
and LSP `userConfig` support, and rust-analyzer on PATH. Supported architectures: x64 and ARM64.
The SDK builds the launcher once; its .NET 10 runtime runs subsequent LSP sessions.

```sh
rustup component add rust-analyzer rust-src
claude plugin marketplace add kjanat/agent-plugins
claude plugin install rust-analyzer@kjanat --scope local
claude plugin disable rust-analyzer-lsp@claude-plugins-official --scope local
claude --init-only
```

Run these commands in the project where you want protection. Disable other plugins claiming
`.rs`, including `rust-guard@local-rust-guard` if you used the earlier standalone setup.
Restart Claude after switching. For an unpublished checkout, use its absolute directory as
the marketplace source instead of `kjanat/agent-plugins`.

`claude --init-only` runs the plugin's Setup hook, publishing the launcher into the persistent
plugin data directory under `runtime/0.2.0`. Build intermediates stay under its `build/0.2.0`
directory. Repeat initialization after plugin updates. No PowerShell launcher, Framework
compiler, or per-LSP compilation remains. Setup needs access to the SDK's restore sources.

If setup fails, check `dotnet --version` (10 or newer), then run `claude --init-only --debug`
to see hook errors. Setup-hook failures do not block Claude itself. A missing compiled guard
prevents this LSP from starting; there is no unguarded fallback.

To test a local plugin without registering a marketplace:

```sh
claude --plugin-dir /absolute/path/to/agent-plugins/plugins/rust-analyzer --init-only
claude --plugin-dir /absolute/path/to/agent-plugins/plugins/rust-analyzer
```

## Configuration

Use the plugin configuration in `/config`, or set a budget at installation:

```sh
claude plugin install rust-analyzer@kjanat --scope local --config memory_limit_mib=4096
```

| Option             | Default         | Meaning                                                         |
| ------------------ | --------------- | --------------------------------------------------------------- |
| `memory_limit_mib` | `6144`          | Windows commit ceiling; Linux/macOS RSS threshold, in whole MiB |
| `server`           | `rust-analyzer` | Executable on PATH, or an absolute executable path              |

Budgets must be whole numbers from 32 to 1,048,576 MiB. Small budgets support bounded tests;
they are not useful for normal Rust work. Relative executable paths and relative/empty PATH
entries are rejected or ignored. Windows requires a native `.exe`; `.exe` is added when
resolving a bare PATH name. The rustup proxy respects the workspace toolchain.

Two analysis workers, at most two Cargo jobs, and disabled eager cache priming reduce
background work. Navigation, diagnostics, proc macros and build scripts remain enabled.
These settings may slow analysis; they are separate from memory enforcement.

## Failure behavior

On **Windows**, the guard assigns the server to a Job Object atomically at process creation.
Descendants inherit membership and cannot break away. Windows rejects allocations that exceed
the combined commit ceiling. Every 100 ms the guard checks the job's peak usage; at 95% it
terminates the entire job with exit code 137. A denied allocation can make the server exit
first, in which case its exit code is preserved. Closing or killing the guard closes the job
handle and removes remaining children, including after normal server exit.

On **Linux/macOS**, an inner supervisor establishes its own process group before launching the
server. A private control pipe reports readiness and the server's exit code; LSP stdin,
stdout and stderr remain inherited byte streams. Every 100 ms the outer supervisor sums
member RSS, including the inner supervisor. At the budget it kills that group and returns
137. The inner supervisor remains alive until cleanup and checks its parent's identity every
50 ms, cleaning up the group if the outer supervisor dies. Accounting errors fail closed.

Unix limitations: polling permits overshoot; shared resident pages may be counted more than
once; swapped/nonresident memory is excluded. Descendants that deliberately create a new
session or process group escape this watchdog. Killing the inner supervisor separately can
also interfere with cleanup. This is best-effort protection for ordinary LSP children, not
kernel-enforced containment or a security sandbox.

Both backends clean up remaining children after normal server exit, preserve normal exit
codes and keep diagnostics off LSP stdout. Crash restarts are disabled to avoid repeatedly
launching the same oversized workload. After a budget stop, Rust intelligence stays unavailable
until restarted; reduce workspace load or choose a suitable budget before retrying.

The default 6 GiB budget applies to one LSP group. It neither reserves system headroom nor
caps other applications or simultaneous sessions. It does not fix Bun's crash handling or
restore terminal modes after an unrelated Claude crash.

## Development and tests

`native/Guard.cs` resolves configuration, `WindowsGuard.cs` owns Job Object containment, and
`UnixGuard.cs` owns process-group supervision. Native calls use `LibraryImport` source
generation. Both projects target .NET 10 and treat compiler warnings as errors.

```sh
claude plugin validate --strict plugins/rust-analyzer
python plugins/rust-analyzer/tests/test_guard.py
python plugins/rust-analyzer/tests/test_guard.py --lsp
```

Python 3.10+ tests execute the actual Setup-hook build command and configured LSP entrypoint
in temporary directories. A small compiled helper exercises binary stdio, quoting, memory
accounting, group termination, cleanup after guard death and normal exit, and invalid settings.
Windows additionally verifies the kernel denies a 512 MiB allocation under a 192 MiB budget.
Unix additionally checks immediate exits and permission-denied startup. Test children expire
after a few seconds if cleanup fails. `--lsp` initializes and shuts down the installed
rust-analyzer on an empty workspace; it does not index a large repository.

Verified locally on Windows x64 and Linux x64 (WSL), with a real rust-analyzer handshake on
Windows. macOS and ARM64 have not been tested locally. The GitHub Actions workflow runs the
containment tests on Windows, Linux and macOS when pushed; no hosted run is claimed here.

## References

- [Windows job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information)
- [Atomic job assignment](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812)
- [Unix process groups](https://man7.org/linux/man-pages/man2/setpgid.2.html)
- [Claude Setup hooks](https://code.claude.com/docs/en/hooks#setup)
- [Claude LSP configuration](https://code.claude.com/docs/en/plugins-reference#lspservers)

<!-- markdownlint-disable-file -->
