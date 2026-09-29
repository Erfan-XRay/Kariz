#!/usr/bin/env python3
"""Drives the kariz-manager menu through a real terminal (a pty), the way a person does.

Checks what only a terminal shows: every question is printed before its answer is read,
numbered choices, port lists and IPv6 addresses in the wizard, the tunnel list is not
taken for a tunnel name, and Ctrl+C: during an action it returns to the menu at once
(no Enter needed), at the menu it leaves. Usage:
sudo python3 tests/manager_tty.py [command...] (default: kariz-manager).
"""

import os
import pty
import re
import select
import sys
import time

ANSI = re.compile(rb"\x1b\[[0-9;]*m")
TIMEOUT = 20
MENU = "Install or update Kariz"


class Session:
    def __init__(self, argv):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execvp(argv[0], argv)
        self.seen = b""

    def expect_any(self, *texts):
        """Waits until one of `texts` appears after what was matched before. Returns the
        one that came first and the output up to and including it."""
        deadline = time.monotonic() + TIMEOUT
        wants = [t.encode() for t in texts]
        while True:
            clean = ANSI.sub(b"", self.seen)
            found = [(clean.find(w), w) for w in wants if clean.find(w) >= 0]
            if found:
                at, want = min(found)
                self.seen = clean[at + len(want):]
                return want.decode(), clean[: at + len(want)].decode(errors="replace")
            left = deadline - time.monotonic()
            if left <= 0:
                self.fail(f"none of {texts!r} on the terminal")
            ready, _, _ = select.select([self.fd], [], [], left)
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 4096)
            except OSError:
                chunk = b""
            if not chunk:
                self.fail(f"the manager ended before printing {texts!r}")
            sys.stdout.write(chunk.decode(errors="replace"))
            sys.stdout.flush()
            self.seen += chunk

    def expect(self, text):
        return self.expect_any(text)[1]

    def send(self, reply):
        os.write(self.fd, reply.encode() + b"\r")

    def answer(self, question, reply):
        self.expect(question)
        self.send(reply)

    def pick(self, heading, reply):
        """A numbered choice: its heading, then its own `Choose` prompt."""
        self.expect(heading)
        self.answer("Choose", reply)

    def ctrl_c(self):
        time.sleep(0.5)
        os.write(self.fd, b"\x03")

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

    def back_in_menu(self):
        """After Ctrl+C: the menu again, without an Enter in between."""
        _, before = self.expect_any(MENU)
        if "Enter: back to the menu" in before:
            self.fail("Ctrl+C asked for Enter before the menu")


def main():
    s = Session(sys.argv[1:] or ["kariz-manager"])

    # 2: a new tunnel through the wizard, with a few wrong answers on the way.
    s.expect(MENU)
    s.answer("Choose", "2")
    s.answer("Name for this tunnel", "tty")
    s.pick("This server is the", "1")
    s.pick("Who connects to whom", "reverse")
    s.pick("Transport", "9")
    s.answer("Type a number from 1 to", "tcpmux")
    s.pick("Profile", "")
    s.answer("Port for the tunnel", "99999")
    s.expect("A port is a number")
    s.send("3091")
    which, _ = s.expect_any("Accept the other server over", "Address the other server connects to")
    if which.startswith("Accept"):
        s.answer("Choose", "")
        s.expect("Address the other server connects to")
    s.answer("Choose", "other")
    s.answer("This server's address", "2001:db8::1")
    s.pick("Token", "")
    s.pick("Protocol", "tcp")
    s.answer("Target host", "")
    s.answer("Ports", "443x")
    s.expect("is not a port")
    s.send("18084,18085-18086=18081")
    s.expect("3 port(s) added")
    s.answer("Add more ports", "y")
    s.pick("Protocol", "udp")
    s.answer("Target host", "::1")
    s.answer("Ports", "18087")
    s.expect("1 port(s) added")
    s.answer("Add more ports", "")
    s.expect("Summary")
    s.expect("[2001:db8::1]:3091")
    s.answer("Create this tunnel", "")
    s.expect("Tunnel 'tty' is running")
    s.expect("--remote [2001:db8::1]:3091")
    s.answer("Enter: back to the menu", "")

    # Ctrl+C in the middle of the wizard: straight back to the menu, nothing written.
    s.expect(MENU)
    s.answer("Choose", "2")
    s.answer("Name for this tunnel", "cancelled")
    s.expect("This server is the")
    s.ctrl_c()
    s.back_in_menu()
    if os.path.exists("/etc/kariz/cancelled.toml"):
        s.fail("the cancelled wizard wrote a tunnel")

    # 4: pick the tunnel by its number, restart it.
    s.answer("Choose", "4")
    listing = s.expect("Tunnel (number or name)")
    number = re.search(r"(\d+)\) tty\s", listing)
    if not number:
        s.fail("the tunnel 'tty' is not in the list")
    s.send(number.group(1))
    s.pick("Action", "restart")
    s.expect("Restarted 'tty'")
    s.answer("Enter: back to the menu", "")

    # 5: follow the log, leave it with Ctrl+C, and land back in the menu at once.
    s.expect(MENU)
    s.answer("Choose", "5")
    s.answer("Tunnel (number or name)", "tty")
    s.expect("Ctrl+C to stop following the log")
    s.ctrl_c()
    s.back_in_menu()

    # 8: remove it, confirming by hand.
    s.answer("Choose", "8")
    s.answer("Tunnel (number or name)", "tty")
    s.answer("Remove tunnel 'tty'", "y")
    s.expect("Removed 'tty'")
    s.answer("Enter: back to the menu", "")

    # Ctrl+C at the menu itself leaves the manager.
    s.expect(MENU)
    s.expect("Choose")
    s.ctrl_c()
    code = s.wait()
    if code != 0:
        s.fail(f"the manager exited with {code}")
    print("\n\nmenu through a terminal: ok")


if __name__ == "__main__":
    main()
