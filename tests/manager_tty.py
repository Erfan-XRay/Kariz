#!/usr/bin/env python3
"""Drives the kariz-manager menu through a real terminal (a pty), the way a person does.

Checks what only a terminal shows: the menu is drawn and a number jumps to its item, a screen
ends with "press any key", and Ctrl+C: during a screen it returns to the menu at once (no key
needed), at the menu it leaves, as q does. Usage:
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
# The menu's own key line: always on the screen (the list scrolls on a small terminal, so an
# item's name may not be), and no screen of an action has it.
MENU = "a number jumps"


class Session:
    def __init__(self, argv):
        # The menu looks for a newer release at start; a test does not wait for GitHub.
        os.environ["KARIZ_NO_UPDATE_CHECK"] = "1"
        os.environ["KARIZ_NO_OFFER"] = "1"
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

    def key(self, text):
        """Keys as they are typed: no Enter is added (the menu reacts to each key)."""
        time.sleep(0.3)
        os.write(self.fd, text.encode())

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
        """After Ctrl+C: the menu again, without a key in between."""
        _, before = self.expect_any(MENU)
        if "Press any key" in before or "Enter: back to the menu" in before:
            self.fail("Ctrl+C asked for a key before the menu")


def main():
    argv = sys.argv[1:] or ["kariz-manager"]
    s = Session(argv)

    # The menu is drawn with its first item highlighted; a number jumps to its item (and shows
    # what it does), without Enter.
    s.expect(MENU)
    s.key("9")
    s.expect("Disconnect from the panel")

    # 2 and Enter: the status screen, and "press any key" before the menu comes back.
    s.key("2")
    s.key("\r")
    s.expect("Press any key to continue")
    s.key(" ")

    # Ctrl+C inside a screen (10 is the logs, which asks which log first): straight back to
    # the menu, no key in between.
    s.expect(MENU)
    s.key("1")
    s.key("0")
    s.key("\r")
    s.expect("Which log?")
    s.ctrl_c()
    s.back_in_menu()

    # The old tunnel entries are gone: tunnels are made in the panel.
    if b"New tunnel" in s.seen:
        s.fail("the menu still offers tunnels")

    # q leaves the manager.
    s.key("q")
    code = s.wait()
    if code != 0:
        s.fail(f"the manager exited with {code} after q")

    # Ctrl+C at the menu itself leaves it too.
    s = Session(argv)
    s.expect(MENU)
    s.ctrl_c()
    code = s.wait()
    if code != 0:
        s.fail(f"the manager exited with {code}")
    print("\n\nmenu through a terminal: ok")


if __name__ == "__main__":
    main()
