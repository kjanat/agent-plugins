# rust-analyzer

Rust code intelligence for Claude Code on **Windows, Linux and macOS**, with a compiled
native Rust supervisor. Claude stays outside the guarded LSP group.

| Platform      | Protection                                                           |
| ------------- | -------------------------------------------------------------------- |
| Windows 10+   | Kernel-enforced committed-memory ceiling for the entire process tree |
| Linux / macOS | Soft watchdog over process-group resident memory (RSS)               |

The Unix watchdog is **not a hard limit**. Fast allocations can exceed the budget before the
next check. It does not use Linux cgroups or macOS virtual-address-space limits.

## Install

Requires **Rust 1.99+ with Cargo and a native linker**, Claude Code with plugin Setup hooks
and LSP `userConfig` support, and rust-analyzer on PATH. Supported architectures: x64 and ARM64.
Cargo builds the guard once; subsequent LSP sessions launch the native executable directly.
There is no .NET SDK/runtime dependency. Python is needed only for tests.

Use a Rust toolchain installed through rustup. Windows MSVC needs the Visual Studio C++ build
tools; Linux needs a C linker/toolchain; macOS needs the Xcode Command Line Tools. These are
the usual prerequisites for building native Rust projects.

```sh
rustup component add rust-analyzer rust-src
claude plugin marketplace add kjanat/agent-plugins
claude plugin install rust-analyzer@kjanat --scope local
claude plugin disable rust-analyzer-lsp@claude-plugins-official --scope local
claude --init-only
```

Run these commands in the project where you want protection. Disable other plugins claiming
`.rs`, including `rust-guard@local-rust-guard` if you used the earlier standalone setup.
Restart Claude after switching.

`claude --init-only` runs the plugin's Setup hook: Cargo fetches the complete
`https://github.com/kjanat/agent-plugins.git` repository's `master` branch and installs the
`rust-analyzer-guard` package with `--locked`. This retains the root Cargo workspace even
when Claude caches only the plugin directory. The executable goes under `runtime/0.3.0/bin`
in the persistent plugin data directory; build intermediates stay under `build/0.3.0`.
Repeat initialization after updates. Dependencies use the fetched workspace-root `Cargo.lock`;
the guard source follows the moving `master` branch, independently of the cached plugin version.
Setup needs GitHub access and, for uncached dependencies, registry access. The configured
command resolves the `.exe` suffix on Windows.

If setup fails, check `rustc --version` and `cargo --version`, then run `claude --init-only --debug`
to see hook errors. Setup-hook failures do not block Claude itself. A missing compiled guard
prevents this LSP from starting; there is no unguarded fallback.

To load a local plugin without registering a marketplace:

```sh
claude --plugin-dir /absolute/path/to/agent-plugins/plugins/rust-analyzer --init-only
claude --plugin-dir /absolute/path/to/agent-plugins/plugins/rust-analyzer
```

The Setup hook still builds GitHub's `master`, including with `--plugin-dir`; unpushed Rust
changes are not installed by initialization. The tests below build an isolated Git snapshot
of the current checkout, including uncommitted changes. Push the workspace changes to `master`
before using the Git-based Setup hook against GitHub.

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
server. A private Unix socket reports readiness and the server's exit code; LSP stdin,
stdout and stderr remain inherited byte streams. Every 100 ms the outer supervisor sums
member RSS, including the inner supervisor. At the budget it kills that group and returns
137. The inner supervisor remains alive until cleanup and checks its parent's identity every
50 ms, cleaning up the group if the outer supervisor dies. Accounting errors fail closed.

Unix limitations: polling permits overshoot; shared resident pages may be counted more than
once; swapped/nonresident memory is excluded. Descendants that deliberately create a new
session or process group escape this watchdog. Simultaneously killing both supervisors can
prevent cleanup. This is best-effort protection for ordinary LSP children, not
kernel-enforced containment or a security sandbox.

Both backends clean up remaining children after normal server exit, preserve normal exit
codes and keep diagnostics off LSP stdout. Crash restarts are disabled to avoid repeatedly
launching the same oversized workload. After a budget stop, Rust intelligence stays unavailable
until restarted; reduce workspace load or choose a suitable budget before retrying.

The default 6 GiB budget applies to one LSP group. It neither reserves system headroom nor
caps other applications or simultaneous sessions. It does not fix Bun's crash handling or
restore terminal modes after an unrelated Claude crash.

## Development and tests

`native/src/main.rs` resolves configuration, `windows.rs` owns Job Object containment, and
`unix.rs` owns process-group supervision. Windows APIs use Microsoft's `windows-sys` bindings;
Unix APIs use `libc`. Linux reads resident pages from `/proc`; macOS reads task information
through libproc. Raw LSP streams are inherited without text decoding or serialization.

```sh
claude plugin validate --strict plugins/rust-analyzer
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python plugins/rust-analyzer/tests/test_guard.py
python plugins/rust-analyzer/tests/test_guard.py --lsp
```

Python 3.10+ and Git are required for tests. Tests snapshot the working checkout into a temporary
Git repository and copy just the plugin subtree into an isolated cache. They execute the actual
Setup hook with only its Git URL redirected to that snapshot, then the configured LSP entrypoint.
Temporary Git, Cargo and installation directories are removed afterward.
A small compiled helper exercises binary stdio, quoting, memory
accounting, group termination, cleanup after guard death and normal exit, and invalid settings.
Windows additionally verifies the kernel denies a 512 MiB allocation under a 192 MiB budget.
Unix additionally checks immediate exits, permission-denied startup and unexpected inner-supervisor
death. Test children expire
after a few seconds if cleanup fails. `--lsp` initializes and shuts down the installed
rust-analyzer on an empty workspace; it does not index a large repository.

Verified locally on Windows x64 and Linux x64 (WSL), with a real rust-analyzer handshake on
Windows. macOS ARM64 receives a cross-target compiler/Clippy check, but has not been run locally.
The GitHub Actions workflow runs formatting, Clippy and containment tests on Windows, Linux and
macOS when pushed; no hosted run is claimed here.

## References

- [Windows job memory limits](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information)
- [Atomic job assignment](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812)
- [Unix process groups](https://man7.org/linux/man-pages/man2/setpgid.2.html)
- [Claude Setup hooks](https://code.claude.com/docs/en/hooks#setup)
- [Claude LSP configuration](https://code.claude.com/docs/en/plugins-reference#lspservers)

<!-- markdownlint-disable-file -->
