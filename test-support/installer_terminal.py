"""Drive the installer through a real terminal without depending on ANSI styling."""

import codecs
import errno
import fcntl
import os
import pty
import re
import select
import signal
import struct
import termios
import time


UP = b"\x1b[A"
DOWN = b"\x1b[B"
LEFT = b"\x1b[D"
RIGHT = b"\x1b[C"
ENTER = b"\r"
ESCAPE = b"\x1b"
PLUGIN_PROMPT = "Installing `leanck` as a plugin. Continue?"
STANDALONE_PROMPT = "Installing `leanck` as standalone. Continue?"


class Screen:
    """The small subset of VT100 used by the installer and Bash readline."""

    def __init__(self, rows, columns):
        self.rows, self.columns = rows, columns
        self.lines = [""] * rows
        self.row = self.column = 0
        self.cursor_visible = self.wrap = True
        self.bracketed_paste = False
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def feed(self, chunk):
        self.pending += self.decoder.decode(chunk)
        while self.pending:
            if self.pending.startswith("\x1b"):
                match = re.match(r"\x1b\[([0-?]*)([ -/]*)([@-~])", self.pending)
                if not match:
                    if len(self.pending) == 1 or self.pending.startswith("\x1b["):
                        break
                    self.pending = self.pending[2:]
                    continue
                self.control(match[1], match[3])
                self.pending = self.pending[match.end():]
                continue
            char, self.pending = self.pending[0], self.pending[1:]
            if char == "\r":
                self.column = 0
            elif char == "\n":
                self.newline()
            elif char == "\b":
                self.column = max(0, self.column - 1)
            elif char == "\t":
                self.column = min(self.columns - 1, (self.column // 8 + 1) * 8)
            elif char >= " ":
                if self.column >= self.columns:
                    if self.wrap:
                        self.newline()
                        self.column = 0
                    else:
                        self.column = self.columns - 1
                line = self.lines[self.row].ljust(self.column)
                self.lines[self.row] = line[:self.column] + char + line[self.column + 1:]
                self.column += 1

    def newline(self):
        self.row += 1
        if self.row == self.rows:
            self.lines.pop(0)
            self.lines.append("")
            self.row -= 1

    def control(self, params, command):
        if params.startswith("?"):
            enabled = command == "h"
            for value in params[1:].split(";"):
                if value == "25":
                    self.cursor_visible = enabled
                elif value == "7":
                    self.wrap = enabled
                elif value == "2004":
                    self.bracketed_paste = enabled
            return
        values = [int(value or 0) for value in params.split(";")]
        count = values[0] or 1
        if command == "A":
            self.row = max(0, self.row - count)
        elif command == "B":
            self.row = min(self.rows - 1, self.row + count)
        elif command == "C":
            self.column = min(self.columns - 1, self.column + count)
        elif command == "D":
            self.column = max(0, self.column - count)
        elif command in ("H", "f"):
            self.row = min(self.rows - 1, count - 1)
            self.column = max(0, (values[1] if len(values) > 1 else 1) - 1)
        elif command == "K":
            if values[0] == 2:
                self.lines[self.row] = ""
            else:
                self.lines[self.row] = self.lines[self.row][:self.column]
        elif command == "J":
            if values[0] == 2:
                self.lines = [""] * self.rows
            else:
                self.lines[self.row] = self.lines[self.row][:self.column]
                self.lines[self.row + 1:] = [""] * (self.rows - self.row - 1)

    def section(self, title):
        starts = [i for i, line in enumerate(self.lines) if title in line]
        return self.lines[starts[-1] + 1:] if starts else []

    def selected(self, title):
        return next((line[2:].strip() for line in self.section(title) if line.startswith("> ")), "")


class InstallerTerminal:
    def __init__(self, case, script, args, env, stdout_pipe=False, size=(32, 120)):
        self.case = case
        self.screen = Screen(*size)
        self.transcript = bytearray()
        self.eof = False
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            command = 'script=$1; installer_shell=$2; shift 2; cat "$script" | "$installer_shell" -s -- "$@"'
            if stdout_pipe:
                command += " | cat"
            os.execve("/bin/sh", ["sh", "-c", command, "installer-test", str(script),
                                  env.get("LEANCK_INSTALLER_SHELL", "sh"), *args], env)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", *size, 0, 0))
        self.initial_flags = termios.tcgetattr(self.master)[3]

    def read(self, timeout=0.05):
        if self.eof or not select.select([self.master], [], [], timeout)[0]:
            return False
        try:
            chunk = os.read(self.master, 65536)
        except OSError as exc:
            if exc.errno != errno.EIO:
                raise
            chunk = b""
        if not chunk:
            self.eof = True
            return False
        self.transcript.extend(chunk)
        self.screen.feed(chunk)
        return True

    def wait(self, predicate, description):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            self.read()
            if predicate():
                # Wait for the entire redraw before sending the next key.
                while self.read(0.01):
                    pass
                if predicate():
                    return
            if self.eof:
                break
        self.case.fail(f"Waiting for {description}\nScreen:\n" + "\n".join(self.screen.lines)
                       + "\nTranscript:\n" + self.transcript.decode(errors="replace"))

    def send(self, data):
        os.write(self.master, data.encode() if isinstance(data, str) else data)

    def choose(self, title, option):
        self.wait(lambda: self.screen.selected(title) and not self.screen.cursor_visible, title)
        for _ in range(8):
            selected = self.screen.selected(title)
            if selected == option:
                self.send(ENTER)
                return
            self.send(DOWN)
            self.wait(lambda: self.screen.selected(title) != selected, f"next option in {title}")
        self.case.fail(f"Option {option!r} not found in {title!r}")

    def prompt(self, label, answer=""):
        self.wait(lambda: label in "\n".join(self.screen.lines[max(0, self.screen.row - 3):self.screen.row + 1])
                  and self.screen.lines[self.screen.row].rstrip().endswith(":"), label)
        self.send(answer + "\n")

    def focus(self, label):
        title = "Advanced configuration"
        self.wait(lambda: self.screen.selected(title) and not self.screen.cursor_visible, title)
        for _ in range(60):
            selected = self.screen.selected(title)
            if selected.split(":", 1)[0].strip() == label:
                return
            self.send(DOWN)
            self.wait(lambda: self.screen.selected(title) != selected, f"field after {selected}")
        self.case.fail(f"Field {label!r} not found")

    def value(self):
        return self.screen.selected("Advanced configuration").partition(":")[2].strip()

    def edit(self, label, value):
        self.focus(label)
        self.send(ENTER)
        self.wait(lambda: self.screen.cursor_visible, f"editing {label}")
        self.send(b"\x15" + str(value).encode() + ENTER)
        self.wait(lambda: not self.screen.cursor_visible, f"saving {label}")

    def enum(self, label, value):
        self.focus(label)
        for _ in range(8):
            previous = self.value()
            if previous == value:
                return
            self.send(ENTER)
            self.wait(lambda: self.value() != previous, f"cycling {label}")
            self.case.assertFalse(self.screen.cursor_visible)
            self.case.assertEqual(self.screen.selected("Advanced configuration").split(":", 1)[0].strip(), label)
        self.case.fail(f"Value {value!r} not found for {label!r}")

    def action(self, label):
        self.focus(label)
        self.send(ENTER)

    def finish(self):
        self.wait(lambda: self.eof, "installer exit")
        _, status = os.waitpid(self.pid, 0)
        self.pid = None
        restored = termios.tcgetattr(self.master)[3]
        mask = termios.ECHO | termios.ICANON | termios.ISIG
        self.case.assertEqual(restored & mask, self.initial_flags & mask)
        self.case.assertTrue(self.screen.cursor_visible)
        self.case.assertTrue(self.screen.wrap)
        self.case.assertFalse(self.screen.bracketed_paste)
        return os.waitstatus_to_exitcode(status), bytes(self.transcript)

    def close(self):
        if self.pid is not None:
            # pty.fork creates a session: reap the shell and every unfinished child.
            try:
                os.killpg(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(self.pid, 0)
        os.close(self.master)
