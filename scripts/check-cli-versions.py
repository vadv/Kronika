#!/usr/bin/env python3
"""Run all shipped --version commands without configuration or writable storage."""

import argparse
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tempfile


BINARIES = tuple(f"kronika-{name}" for name in ("collector", "web", "dump", "report", "demo"))
TIMEOUT_SECONDS = 5


def run(command, cwd, environment, trace=None):
    privileges = {}
    if os.geteuid() == 0:
        privileges = {"user": 65534, "group": 65534, "extra_groups": []}
    with subprocess.Popen(
        command,
        cwd=cwd,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
        pass_fds=() if trace is None else (trace.fileno(),),
        **privileges,
    ) as child:
        try:
            stdout, stderr = child.communicate(timeout=TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise AssertionError(f"{command}: did not exit within {TIMEOUT_SECONDS} seconds") from None
        return child.returncode, stdout, stderr


def check_trace(trace, storage):
    calls = []
    for line in trace.splitlines():
        match = re.match(r"(?:\d+\s+)?([a-z0-9_]+)\(", line)
        if match:
            calls.append((match[1], line))
    assert sum(name == "execve" for name, _ in calls) == 1, trace
    forbidden = {
        "clone", "clone3", "fork", "vfork", "execveat",
        "socket", "socketpair", "bind", "connect", "listen", "accept", "accept4",
        "sendto", "sendmsg", "sendmmsg", "recvfrom", "recvmsg", "recvmmsg",
        "mkdir", "mkdirat", "unlink", "unlinkat", "rmdir", "rename", "renameat", "renameat2",
        "link", "linkat", "symlink", "symlinkat", "truncate", "chmod", "fchmodat",
        "chown", "lchown", "fchownat", "mknod", "mknodat", "creat",
    }
    for name, line in calls:
        assert name not in forbidden, f"--version performed a startup operation: {line}"
        assert str(storage) not in line, f"--version accessed configured storage: {line}"
        if name in {"open", "openat", "openat2"}:
            assert not re.search(r"O_(WRONLY|RDWR|CREAT|TRUNC|APPEND)", line), line


def check(binary, version, root, strace):
    cwd = root / "read-only"
    storage = cwd / "storage-must-not-be-accessed"
    invalid = {
        "KRONIKA_STORAGE_DIR": str(storage),
        "KRONIKA_INTERVAL_S": "not-a-number",
        "KRONIKA_PG_DSNS": "invalid-postgresql-dsn",
        "KRONIKA_WEB_LISTEN": "not-an-address",
        "KRONIKA_WEB_SOURCES": "not-a-number",
        "KRONIKA_WEB_AUTH": "invalid",
        "KRONIKA_WEB_USER": "",
        "KRONIKA_WEB_PASSWORD": "",
        "KRONIKA_DEMO_DIR": str(storage / "demo"),
        "KRONIKA_DEMO_DURATION_S": "not-a-number",
        "KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED": "invalid",
        "KRONIKA_COLLECTOR_BIN": str(storage / "collector-must-not-start"),
        # Tokio rejects zero worker threads when its runtime is constructed.
        "TOKIO_WORKER_THREADS": "0",
        "TMPDIR": str(storage / "temporary-files"),
    }
    expected = (binary.name + " " + version + "\n").encode()
    for label, environment in (("empty environment", {}), ("invalid configuration", invalid)):
        command = [str(binary), "--version"]
        with tempfile.TemporaryFile(mode="w+b", dir=root) as trace:
            if strace:
                if os.geteuid() == 0:
                    os.fchown(trace.fileno(), 65534, 65534)
                command = [
                    strace, "-f", "-qq", "-s", "256", "-e", "trace=%process,%network,%file",
                    "-o", f"/proc/self/fd/{trace.fileno()}", "--", *command,
                ]
            status, stdout, stderr = run(command, cwd, environment, trace if strace else None)
            assert (status, stdout, stderr) == (0, expected, b""), (
                f"{binary.name} ({label}): status={status}, stdout={stdout!r}, stderr={stderr!r}"
            )
            if strace:
                trace.seek(0)
                check_trace(trace.read().decode(), storage)
        assert list(cwd.iterdir()) == [], f"{binary.name} wrote to its working directory"
        print(f"{binary.name} --version [{label}]: status={status}, stdout={stdout!r}, stderr={stderr!r}"
              + (", no startup syscalls" if strace else ""))

    # Only the standalone option bypasses normal argument/configuration handling.
    invalid["TOKIO_WORKER_THREADS"] = "1"
    for arguments in (["--unknown-option"], ["--version", "--unknown-option"], ["--unknown-option", "--version"]):
        status, stdout, stderr = run([str(binary), *arguments], cwd, invalid)
        assert status != 0 and stdout == b"" and stderr, (
            f"{binary.name} {arguments}: unexpected argument handling: {(status, stdout, stderr)!r}"
        )
        assert expected not in stdout + stderr, f"{binary.name} ignored an extra argument"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="extracted package or build output directory")
    parser.add_argument("--version", help="expected version; defaults to the package BUILDINFO value")
    parser.add_argument("--strace", action="store_true", help="also reject startup syscalls (Linux strace required)")
    arguments = parser.parse_args()
    version = arguments.version
    if version is None:
        metadata = dict(line.split("=", 1) for line in (arguments.directory / "BUILDINFO").read_text().splitlines())
        version = metadata["version"]
    assert version and not any(character.isspace() for character in version), "invalid expected version"
    strace = shutil.which("strace") if arguments.strace else None
    if arguments.strace and strace is None:
        parser.error("--strace requires strace on PATH")
    source = arguments.directory / "bin" if (arguments.directory / "bin").is_dir() else arguments.directory
    with tempfile.TemporaryDirectory(prefix="kronika-cli-version-") as scratch:
        root = Path(scratch)
        # Root-owned extraction directories can be inaccessible to the test uid.
        root.chmod(0o755)
        cwd = root / "read-only"
        cwd.mkdir(mode=0o555)
        try:
            for name in BINARIES:
                binary = root / name
                shutil.copyfile(source / name, binary)
                binary.chmod(0o555)
                check(binary, version, root, strace)
                binary.unlink()
        finally:
            cwd.chmod(0o755)
    print("All five CLI version and argument contracts passed as an unprivileged user.")


if __name__ == "__main__":
    main()
