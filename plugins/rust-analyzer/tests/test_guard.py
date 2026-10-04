"""Windows containment tests through the plugin's actual PowerShell entrypoint.

Run: python plugins/rust-analyzer/tests/test_guard.py
Add --lsp to test installed rust-analyzer initialize/shutdown on an empty workspace.
"""

import argparse
import base64
import ctypes
import json
import os
import queue
import subprocess
import tempfile
import threading
from pathlib import Path

PLUGIN = Path(__file__).resolve().parents[1]
CONFIG = json.loads((PLUGIN / ".lsp.json").read_text())["rust-analyzer"]
MANIFEST = json.loads((PLUGIN / ".claude-plugin/plugin.json").read_text())
COMMAND = [
    CONFIG["command"],
    *[
        argument.replace("${CLAUDE_PLUGIN_ROOT}", str(PLUGIN))
        for argument in CONFIG["args"]
    ],
]


def environment(server, limit="96"):
    result = dict(os.environ)
    result.update(
        RUST_ANALYZER_EXECUTABLE=str(server), RUST_ANALYZER_MEMORY_LIMIT_MIB=limit
    )
    return result


def stopped(pid):
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
    helper = directory / "child with spaces.exe"
    compiler = (
        Path(os.environ["WINDIR"]) / "Microsoft.NET/Framework64/v4.0.30319/csc.exe"
    )
    subprocess.run(
        [
            str(compiler),
            "/nologo",
            "/warnaserror",
            "/platform:x64",
            "/target:exe",
            "/out:" + str(helper),
            str(PLUGIN / "tests/TestChild.cs"),
        ],
        check=True,
        timeout=5,
    )
    child_environment = environment(helper)

    def run(*args, payload=b"", env=child_environment, cwd=None):
        return subprocess.run(
            [*COMMAND, *args],
            env=env,
            cwd=cwd,
            input=payload,
            capture_output=True,
            timeout=5,
        )

    payload = bytes(range(256)) * 4096
    result = run("echo", payload=payload)
    assert (result.returncode, result.stdout, result.stderr) == (
        7,
        payload,
        b"stderr-only",
    ), result.stderr
    print("PASS binary stdio, stderr separation, EOF and exit code", flush=True)

    # PowerShell -File has its own argv rules; this validates the actual plugin entrypoint.
    arguments = ["a b", 'quote"here', "C:\\space path\\", "雪", '\\"', "tail\\\\"]
    result = run("args", *arguments)
    assert result.returncode == 0, result.stderr
    assert [
        base64.b64decode(line).decode() for line in result.stdout.splitlines()
    ] == arguments, result.stdout
    print("PASS executable path, Unicode and quoted arguments", flush=True)

    result = run("deny")
    assert result.returncode == 42, (result.returncode, result.stderr)
    print("PASS hard cap denies a 128 MiB allocation under a 96 MiB budget", flush=True)

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
    print("PASS killing the PowerShell supervisor removes descendants", flush=True)

    result = run("orphan")
    assert result.returncode == 0, result.stderr
    assert stopped(int(result.stdout))
    print("PASS normal server exit removes leftover descendant", flush=True)

    for server, limit in [
        (directory / "missing.exe", "96"),
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

    for relative in [str(helper)[2:], helper.drive + helper.name]:
        result = run("echo", env=environment(relative), cwd=directory)
        assert result.returncode == 125 and not result.stdout, (
            result.returncode,
            result.stderr,
        )
        assert b"relative paths" in result.stderr, result.stderr
    print(
        "PASS drive-relative and current-drive-rooted server paths rejected", flush=True
    )

    path_environment = environment(helper.name)
    path_environment["PATH"] = str(directory) + os.pathsep + path_environment["PATH"]
    result = run("echo", payload=b"from PATH", env=path_environment)
    assert result.returncode == 7 and result.stdout == b"from PATH", result.stderr
    print("PASS executable resolution from PATH", flush=True)


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
        except Exception as error:
            messages.put(error)

    def send(message):
        body = json.dumps({"jsonrpc": "2.0", **message}).encode()
        process.stdin.write(("Content-Length: %d\r\n\r\n" % len(body)).encode() + body)
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
    if os.name != "nt":
        raise SystemExit("These tests require 64-bit Windows.")
    with tempfile.TemporaryDirectory(prefix="rust-analyzer-guard-") as temporary:
        directory = Path(temporary)
        checks(directory)
        if arguments.lsp:
            lsp_probe(directory)
