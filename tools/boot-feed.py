#!/usr/bin/env python3
"""Paced serial feeder for the headless boot test.

QEMU's serial is attached to this process's pipes. The helper launches the
emulator, copies its serial output to stdout (which CI captures), and drives
the interactive shell from a command file.

Why the feeding is shaped the way it is:

* QEMU only forwards stdin to the guest while the chardev is actively being
  written. A single line sent after the shell's prompt can sit unread forever,
  but a stream that starts at launch is delivered. So the feeder writes from
  the very first moment and keeps writing.
* The shell starts reading at an arbitrary byte offset in that stream, so its
  first line may be a fragment of a command. To recover, the feeder sends the
  command body (the script minus its final `exit`) and watches the transcript.
  Once a full pass has begun (the `help` banner appears), it appends `exit` and
  stops: the shell has already buffered the rest of that pass, so every command
  runs in order before the shell quits.
* A whole-script burst overruns the guest's 16550 receive FIFO and the kernel's
  input ring while the shell is busy on a command, which tears lines. The
  feeder therefore sends one body per observed progress marker (`stat /dev/blk`
  output) with a timeout fallback, so at most one pass is ever in flight and no
  burst is large enough to overflow.

The exit status is QEMU's: the test succeeds when it exits 33 (the loader's
isa-debug-exit value), and the watchdog kills a wedged guest.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import threading
import time


def split_script(path: str, exit_line: str) -> tuple[bytes, bytes]:
    """Returns the fed body and the terminating `exit` line.

    The body is every line except the final `exit`, which the feeder appends
    only after a full pass is confirmed.
    """
    with open(path, "rb") as handle:
        lines = handle.read().splitlines()
    terminator = exit_line.encode()
    body = [line for line in lines if line.strip() != terminator]
    payload = b"\n".join(body) + b"\n"
    return payload, terminator + b"\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--script", required=True, help="command file to drive")
    parser.add_argument(
        "--ready",
        default="Commands: help echo",
        help="marker proving a full command pass has begun",
    )
    parser.add_argument(
        "--progress",
        default="/dev/blk: char device",
        help="marker proving the shell consumed a pass",
    )
    parser.add_argument("--exit-line", default="exit")
    parser.add_argument(
        "--feed-timeout",
        type=float,
        default=4.0,
        help="seconds without progress before re-sending the body",
    )
    parser.add_argument("--watchdog", type=float, default=240.0)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        print("boot-feed: no QEMU command given after --", file=sys.stderr)
        return 2

    payload, exit_line = split_script(args.script, args.exit_line)
    if not payload:
        print("boot-feed: script is empty", file=sys.stderr)
        return 2

    try:
        qemu = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            bufsize=0,
        )
    except OSError as error:
        print(f"boot-feed: cannot launch QEMU: {error}", file=sys.stderr)
        return 2

    done = threading.Event()
    lock = threading.Lock()
    state = {"ready": False, "progress": 0}
    ready = args.ready.encode()
    progress = args.progress.encode()

    def reader() -> None:
        """Copies the guest's serial stream to stdout and tracks markers."""
        out = sys.stdout.buffer
        # A short carry catches markers split across reads without re-counting
        # a marker that already fit in the previous chunk.
        carry = b""
        try:
            while True:
                data = qemu.stdout.read(4096)
                if not data:
                    break
                out.write(data)
                out.flush()
                window = carry + data
                with lock:
                    if ready in window:
                        state["ready"] = True
                    state["progress"] += window.count(progress)
                carry = window[-(len(progress) - 1):]
        except (OSError, ValueError):
            pass
        finally:
            done.set()

    def feeder() -> None:
        """Sends the body until a full pass starts, then appends `exit`."""
        def send(data: bytes) -> bool:
            try:
                qemu.stdin.write(data)
                qemu.stdin.flush()
                return True
            except (OSError, ValueError):
                return False

        if not send(payload):
            return
        last = 0
        deadline = time.monotonic() + args.feed_timeout
        while not done.is_set():
            with lock:
                started = state["ready"]
                seen = state["progress"]
            if started:
                break
            if seen > last:
                last = seen
                if not send(payload):
                    return
                deadline = time.monotonic() + args.feed_timeout
            elif time.monotonic() > deadline:
                if not send(payload):
                    return
                deadline = time.monotonic() + args.feed_timeout
            time.sleep(0.1)
        # Terminate any partial line, then exit. The rest of the confirmed
        # pass is already buffered, so it runs first.
        send(b"\n" + exit_line)

    thread = threading.Thread(target=reader, daemon=True)
    thread.start()
    threading.Thread(target=feeder, daemon=True).start()

    if not done.wait(args.watchdog):
        print("boot-feed: watchdog expired, killing QEMU", file=sys.stderr)
        qemu.kill()
        done.wait(10)
        try:
            qemu.wait(timeout=10)
        except subprocess.TimeoutExpired:
            pass
        return 124

    return qemu.wait()


if __name__ == "__main__":
    raise SystemExit(main())
