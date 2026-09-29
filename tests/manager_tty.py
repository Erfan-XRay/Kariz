#!/usr/bin/env python3
"""Drives the kariz-manager menu through a real terminal (a pty), the way a person does.

The other manager tests give the answers in a file (KARIZ_INPUT); this one checks what
only a terminal shows: that every question is printed before its answer is read, that
the list of tunnels is not taken for a tunnel name, and that Ctrl+C in a log returns to
the menu. Usage: sudo python3 tests/manager_tty.py [command...] (default: kariz-manager).
"""

import os
import pty
import re
import select
import sys
import time

ANSI = re.compile(rb"\x1b\[[0-9;]*m")
TIMEOUT = 20


class Session:
    def __init__(self, argv):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execvp(argv[0], argv)
        self.seen = b""

    def expect(self, text):
        """Waits until `text` appears in the output after what was matched before; returns
        the output up to and including it."""
        deadline = time.monotonic() + TIMEOUT
        want = text.encode()
        while True:
            clean = ANSI.sub(b"", self.seen)
            at = clean.find(want)
            if at >= 0:
                # Keep only what follows the match (colour codes dropped).
                self.seen = clean[at + len(want):]
                return clean[: at + len(want)].decode(errors="replace")
            left = deadline - time.monotonic()
            if left <= 0:
                self.fail(f"no {text!r} on the terminal")
            ready, _, _ = select.select([self.fd], [], [], left)
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 4096)
            except OSError:
                chunk = b""
            if not chunk:
                self.fail(f"the manager ended before printing {text!r}")
            sys.stdout.write(chunk.decode(errors="replace"))
            sys.stdout.flush()
            self.seen += chunk

    def answer(self, question, reply):
        self.expect(question)
        os.write(self.fd, reply.encode() + b"\r")

    def fail(self, why):
        print(f"\n\nFAILED: {why}\nlast output: {ANSI.sub(b'', self.seen)[-400:]!r}")
        sys.exit(1)

    def wait(self):
        deadline = time.monotonic() + TIMEOUT
        while time.monotonic() < deadline:
            try:
                ready, _, _ = select.select([self.fd], [], [], 0.2)
                if ready:
                    os.read(self.fd, 4096)
            except OSError:
                pass
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                return os.waitstatus_to_exitcode(status)
        self.fail("the manager did not exit")


def main():
    s = Session(sys.argv[1:] or ["kariz-manager"])

    # 2: a new tunnel through the wizard, every question answered by hand.
    s.answer("Choose", "2")
    s.answer("Name for this tunnel", "tty")
    s.answer("This server is the", "entry")
    s.answer("Mode", "reverse")
    s.answer("Transport", "bogus")
    s.expect("Pick one of")
    s.answer("Transport", "tcpmux")
    s.answer("Profile", "")
    s.answer("Port the other server connects to", "99999")
    s.expect("A port is a number")
    s.answer("Port the other server connects to", "3091")
    s.answer("public IP", "127.0.0.1")
    s.answer("Generate a new token", "")
    s.answer("Forward rule", "18084=127.0.0.1:18081")
    s.answer("Forward rule", "")
    s.expect("Tunnel 'tty' is running")
    s.answer("Enter to go back to the menu", "")

    # 4: pick the tunnel by its number, restart it.
    s.answer("Choose", "4")
    listing = s.expect("Tunnel (number or name)")
    number = re.search(r"(\d+)\) tty\s", listing)
    if not number:
        s.fail("the tunnel 'tty' is not in the list")
    os.write(s.fd, number.group(1).encode() + b"\r")
    s.answer("Action", "restart")
    s.expect("Restarted 'tty'")
    s.answer("Enter to go back to the menu", "")

    # 5: follow the log, leave it with Ctrl+C, and land back in the menu.
    s.answer("Choose", "5")
    s.answer("Tunnel (number or name)", "tty")
    s.expect("Ctrl+C to stop following the log")
    time.sleep(1)
    os.write(s.fd, b"\x03")
    s.answer("Enter to go back to the menu", "")

    # 8: remove it, confirming by hand.
    s.answer("Choose", "8")
    s.answer("Tunnel (number or name)", "tty")
    s.answer("Remove tunnel 'tty'", "y")
    s.expect("Removed 'tty'")
    s.answer("Enter to go back to the menu", "")

    s.answer("Choose", "0")
    code = s.wait()
    if code != 0:
        s.fail(f"the manager exited with {code}")
    print("\n\nmenu through a terminal: ok")


if __name__ == "__main__":
    main()
