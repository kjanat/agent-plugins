"""Containment tests through the plugin's native Rust entrypoint.

Run: python plugins/rust-analyzer/tests/test_guard.py
Add --lsp to test installed rust-analyzer initialize/shutdown on an empty workspace.
"""

import argparse
import ctypes
import json
import os
import queue
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path

PLUGIN = Path(__file__).resolve().parents[1]
CONFIG = json.loads((PLUGIN / ".lsp.json").read_text())["rust-analyzer"]
MANIFEST = json.loads((PLUGIN / ".claude-plugin/plugin.json").read_text())
COMMAND = []
WINDOWS = os.name == "nt"


def install_from_git(directory):
    # Snapshot working files, including unstaged edits, without touching the index.
    repository = PLUGIN.parents[1]
    source = directory / "git source"
    source.mkdir()
    files = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=repository,
        capture_output=True,
        check=True,
        timeout=5,
    ).stdout
    for name in set(files.split(b"\0")) - {b""}:
        relative = Path(os.fsdecode(name))
        original = repository / relative
        if original.is_file():
            destination = source / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(original, destination)

    # A cached plugin has no ancestor Cargo workspace. Exercise exactly that layout.
    cached = directory / "cached plugin"
    shutil.copytree(source / PLUGIN.relative_to(repository), cached)
    hook = json.loads((cached / "hooks/hooks.json").read_text())["hooks"]["Setup"][0][
        "hooks"
    ][0]
    arguments = list(hook["args"])
    branch = arguments[arguments.index("--branch") + 1]
    git = [
        "git",
        "-c",
        "core.hooksPath=" + str(directory / "no-hooks"),
        "-c",
        "commit.gpgSign=false",
        "-c",
        "user.name=Guard test fixture",
        "-c",
        "user.email=guard-test@example.invalid",
    ]
    for operation in (
        ["init", "--quiet", "--initial-branch", branch],
        ["add", "--force", "."],
        ["commit", "--quiet", "-m", "Test workspace snapshot"],
    ):
        subprocess.run([*git, *operation], cwd=source, check=True, timeout=10)

    replacements = {
        "${CLAUDE_PLUGIN_ROOT}": str(cached),
        "${CLAUDE_PLUGIN_DATA}": str(directory / "plugin-data"),
    }

    def expand(value):
        for key, replacement in replacements.items():
            value = value.replace(key, replacement)
        return value

    # Change only the Git URL: build the current snapshot with the actual hook flags.
    arguments[arguments.index("--git") + 1] = source.as_uri()
    build_environment = dict(os.environ, CARGO_HOME=str(directory / "cargo-home"))
    subprocess.run(
        [hook["command"], *map(expand, arguments)],
        cwd=cached,
        env=build_environment,
        check=True,
        timeout=hook["timeout"],
    )
    installed = json.loads((cached / ".lsp.json").read_text())["rust-analyzer"]
    print("PASS Git workspace installation from isolated cached plugin", flush=True)
    return [expand(installed["command"]), *map(expand, installed.get("args", []))]


def environment(server, limit="192"):
    result = dict(os.environ)
    result.update(
        RUST_ANALYZER_EXECUTABLE=str(server), RUST_ANALYZER_MEMORY_LIMIT_MIB=limit
    )
    return result


def stopped(pid):
    if not WINDOWS:
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            result = subprocess.run(
                ["ps", "-o", "stat=", "-p", str(pid)],
                capture_output=True,
                timeout=2,
                check=False,
            )
            if result.returncode == 1 or result.stdout.strip().startswith(b"Z"):
                return True
            assert result.returncode == 0, result.stderr
            time.sleep(0.05)
        return False
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.restype = ctypes.c_void_p
    kernel.OpenProcess.argtypes = [ctypes.c_uint, ctypes.c_int, ctypes.c_uint]
    kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint]
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.OpenProcess(0x100000, False, pid)
    if not handle:
        error = ctypes.get_last_error()
        if error == 87:  # ERROR_INVALID_PARAMETER: no process with this PID.
            return True
        raise ctypes.WinError(error)
    try:
        return kernel.WaitForSingleObject(handle, 1000) == 0
    finally:
        kernel.CloseHandle(handle)


def checks(directory):
    helper_directory = directory / "helper with spaces"
    helper_directory.mkdir()
    helper = helper_directory / ("test-child.exe" if WINDOWS else "test-child")
    subprocess.run(
        [
            "rustc",
            "--edition=2024",
            "-Dwarnings",
            str(PLUGIN / "tests/test_child.rs"),
            "-o",
            str(helper),
        ],
        check=True,
        timeout=120,
    )
    child_environment = environment(helper)

    def run(*args, payload=b"", env=child_environment, cwd=None):
        return subprocess.run(
            [*COMMAND, *args],
            check=False,
            env=env,
            cwd=cwd,
            input=payload,
            capture_output=True,
            timeout=5,
        )

    payload = bytes(range(256)) * 4096
    result = run("echo", payload=payload)
    assert result.returncode == 7 and result.stdout == payload, result.stderr
    # Unix startup diagnostics and the child's stderr may arrive in either order.
    assert b"stderr-only" in result.stderr, result.stderr
    if WINDOWS:
        assert result.stderr == b"stderr-only", result.stderr
    print("PASS binary stdio, stderr separation, EOF and exit code", flush=True)

    # Include empty arguments and trailing backslashes through the actual launcher.
    arguments = ["", "a b", 'quote"here', "C:\\space path\\", "雪", '\\"', "tail\\\\"]
    result = run("args", *arguments)
    assert result.returncode == 0, result.stderr
    assert [
        bytes.fromhex(line.decode("ascii")).decode()
        for line in result.stdout.splitlines()
    ] == arguments, result.stdout
    print("PASS executable path, Unicode and quoted arguments", flush=True)

    if WINDOWS:
        result = run("deny")
        assert result.stdout.strip() == b"allocation-denied", (
            result.returncode,
            result.stdout,
            result.stderr,
        )
        assert result.returncode in (42, 137), (result.returncode, result.stderr)
        print(
            "PASS hard cap denies a 512 MiB allocation under a 192 MiB budget",
            flush=True,
        )

    result = run("tree")
    assert result.returncode == 137, (result.returncode, result.stderr)
    assert all(stopped(int(pid)) for pid in result.stdout.split())
    assert b"LSP stopped" in result.stderr
    print(
        "PASS descendant memory triggers whole-group termination; test caller survives",
        flush=True,
    )

    process = subprocess.Popen(
        [*COMMAND, "hold"],
        env=child_environment,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        output = queue.Queue()
        threading.Thread(
            target=lambda: output.put(process.stdout.readline()), daemon=True
        ).start()
        pids = [int(pid) for pid in output.get(timeout=5).split()]
        assert len(pids) == 2
        process.kill()
        process.communicate(timeout=5)
        assert all(stopped(pid) for pid in pids)
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=5)
    print("PASS killing the compiled supervisor removes descendants", flush=True)

    result = run("orphan")
    assert result.returncode == 0, result.stderr
    assert stopped(int(result.stdout))
    print("PASS normal server exit removes leftover descendant", flush=True)

    for server, limit in [
        (directory / "missing.exe", "192"),
        (helper, "1"),
        (helper, "1.5"),
        (helper, "NaN"),
    ]:
        result = run("echo", env=environment(server, limit))
        assert result.returncode == 125 and not result.stdout, (
            result.returncode,
            result.stderr,
        )
    print(
        "PASS missing server and invalid budgets fail closed with clean stdout",
        flush=True,
    )

    for relative in (
        [str(helper)[2:], helper.drive + helper.name]
        if WINDOWS
        else ["./TestChild", "subdir/TestChild"]
    ):
        result = run("echo", env=environment(relative), cwd=directory)
        assert result.returncode == 125 and not result.stdout, (
            result.returncode,
            result.stderr,
        )
        assert b"relative paths" in result.stderr, result.stderr
    print("PASS relative server paths rejected", flush=True)

    path_environment = environment(helper.name)
    path_environment["PATH"] = (
        str(helper_directory) + os.pathsep + path_environment["PATH"]
    )
    result = run("echo", payload=b"from PATH", env=path_environment)
    assert result.returncode == 7 and result.stdout == b"from PATH", result.stderr
    print("PASS executable resolution from PATH", flush=True)

    if not WINDOWS:
        for _ in range(5):
            result = run("args")
            assert result.returncode == 0 and not result.stdout, result.stderr
        print("PASS immediate server exit preserves status", flush=True)
        denied = directory / "not-executable"
        denied.write_text("not executable")
        denied.chmod(0o600)
        result = run(env=environment(denied))
        assert result.returncode == 125 and not result.stdout, result.stderr
        assert b"could not launch" in result.stderr, result.stderr
        print("PASS startup permission failure reports cause and exits 125", flush=True)

        process = subprocess.Popen(
            [*COMMAND, "hold"],
            env=child_environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            output = queue.Queue()
            threading.Thread(
                target=lambda: output.put(process.stdout.readline()), daemon=True
            ).start()
            pids = [int(pid) for pid in output.get(timeout=5).split()]
            assert len(pids) == 2
            result = subprocess.run(
                ["ps", "-o", "ppid=", "-p", str(pids[0])],
                capture_output=True,
                check=True,
                timeout=2,
            )
            inner = int(result.stdout)
            result = subprocess.run(
                ["ps", "-o", "ppid=,pgid=", "-p", str(inner)],
                capture_output=True,
                check=True,
                timeout=2,
            )
            assert inner > 1 and inner != os.getpid()
            assert [int(value) for value in result.stdout.split()] == [
                process.pid,
                inner,
            ]
            os.kill(inner, signal.SIGKILL)
            process.communicate(timeout=5)
            assert process.returncode == 125
            assert all(stopped(pid) for pid in pids)
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=5)
        print(
            "PASS unexpected inner-supervisor death cleans up the LSP group", flush=True
        )


def lsp_probe(directory):
    server = MANIFEST["userConfig"]["server"]["default"]
    limit = str(MANIFEST["userConfig"]["memory_limit_mib"]["default"])
    child_environment = environment(server, limit)
    result = subprocess.run(
        [*COMMAND, "--version"],
        env=child_environment,
        input=b"",
        capture_output=True,
        timeout=5,
        check=True,
    )
    print(result.stdout.decode().strip(), flush=True)
    fixture = directory / "empty-workspace"
    fixture.mkdir()
    process = subprocess.Popen(
        COMMAND,
        env=child_environment,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    messages = queue.Queue()

    def read_messages():
        try:
            while True:
                headers = {}
                while True:
                    line = process.stdout.readline()
                    if not line:
                        raise EOFError("LSP stdout closed")
                    if line == b"\r\n":
                        break
                    key, value = line.decode("ascii").split(":", 1)
                    headers[key.lower()] = value.strip()
                messages.put(
                    json.loads(process.stdout.read(int(headers["content-length"])))
                )
        except (EOFError, KeyError, OSError, OverflowError, ValueError) as error:
            messages.put(error)

    def send(message):
        body = json.dumps({"jsonrpc": "2.0", **message}).encode()
        process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        process.stdin.flush()

    def response(identifier):
        while True:
            message = messages.get(timeout=5)
            if isinstance(message, Exception):
                raise message
            if message.get("id") == identifier and "method" not in message:
                assert "error" not in message, message
                return message["result"]
            if "id" in message and "method" in message:
                send({"id": message["id"], "result": None})

    reader = threading.Thread(target=read_messages, daemon=True)
    reader.start()
    try:
        send({
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": None,
                "rootUri": fixture.as_uri(),
                "capabilities": {},
                "initializationOptions": CONFIG["initializationOptions"],
            },
        })
        assert response(1)["capabilities"]["hoverProvider"]
        send({"method": "initialized", "params": {}})
        send({"id": 2, "method": "shutdown", "params": None})
        response(2)
        send({"method": "exit"})
        process.stdin.close()
        assert process.wait(timeout=5) == 0
        reader.join(timeout=1)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
    print("PASS real rust-analyzer initialize/shutdown on empty workspace", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lsp", action="store_true")
    arguments = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="rust-analyzer-guard-") as temporary:
        directory = Path(temporary)
        COMMAND = install_from_git(directory)
        checks(directory)
        if arguments.lsp:
            lsp_probe(directory)
