"""Bounded subprocess execution for the fleet qualification adapter."""

import os
import signal
import subprocess
import threading
import time


class ProcessError(RuntimeError):
    """A fixed subprocess failed, exceeded its bound, or was cancelled."""


def run(argv, *, cwd=None, data=None, timeout=15, cancelled=lambda: False, env=None):
    """Bound a fixed argv and its process group; this is not an OS sandbox.

    A deliberately detached descendant can escape a POSIX process group. Only
    the audited fixed Buzz and index-reading Git commands use this function.
    """
    if cancelled():
        raise ProcessError("cancelled")
    options = {"start_new_session": True} if os.name != "nt" else {
        "creationflags": subprocess.CREATE_NEW_PROCESS_GROUP
    }
    process = subprocess.Popen(
        argv, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, env=env, **options,
    )
    captured = [bytearray(), bytearray()]
    overflow = threading.Event()

    def read(stream, target):
        try:
            while True:
                block = stream.read(4096)
                if not block:
                    return
                if len(target) + len(block) > 256 * 1024:
                    overflow.set()
                else:
                    target.extend(block)
        finally:
            stream.close()

    readers = [threading.Thread(target=read, args=(stream, target), daemon=True)
               for stream, target in zip((process.stdout, process.stderr), captured)]
    for reader in readers:
        reader.start()
    # All protocol inputs are capped before reaching this function.
    def write():
        try:
            if data:
                process.stdin.write(data.encode("utf-8"))
            process.stdin.close()
        except (BrokenPipeError, OSError):
            pass
    writer = threading.Thread(target=write, daemon=True)
    writer.start()
    reason = None
    deadline = time.monotonic() + timeout
    try:
        while process.poll() is None:
            if cancelled():
                reason = "cancelled"
            elif overflow.is_set():
                reason = "output_limit"
            elif time.monotonic() >= deadline:
                reason = "timeout"
            if reason:
                break
            time.sleep(0.025)
    finally:
        # A child may outlive its parent with inherited output pipes. Always
        # contain the process group, including after the parent exits normally.
        if os.name == "nt":
            if process.poll() is None:
                killed = subprocess.run(
                    ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                    capture_output=True, timeout=5,
                )
                if killed.returncode and process.poll() is None:
                    raise ProcessError("process_tree_containment_failed")
        else:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait(timeout=5)
        for reader in readers:
            reader.join(timeout=5)
        writer.join(timeout=5)
        if any(reader.is_alive() for reader in readers):
            raise ProcessError("process_tree_containment_failed")
    if reason or overflow.is_set():
        raise ProcessError(reason or "output_limit")
    if process.returncode:
        # Do not echo stderr: native CLIs can accidentally include credentials.
        raise ProcessError("subprocess_exit_" + str(process.returncode))
    return captured[0].decode("utf-8", errors="strict")
