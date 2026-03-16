"""Real-terminal coverage for `leanck config init`; invoked by Cargo tests."""

import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest
from installer_terminal import Screen

BINARY = str(Path(sys.argv.pop(1)).resolve()) if __name__ == "__main__" else os.environ.get("LEANCK_TEST_BINARY")
ENTER = b"\r"
UP = b"\x1b[A"
DOWN = b"\x1b[B"
LEFT = b"\x1b[D"
RIGHT = b"\x1b[C"
CLEAR = b"\x04"
ALL = b"\x01"
PREVIOUS = b"\t\r"
NEXT = b"\t\t\r"
ROOT_NEXT = b"\t\r"


class Terminal:
    def __init__(self, case, args, stdout_file=None, size=(40, 140)):
        self.case = case
        self.data = bytearray()
        self.offset = 0
        self.eof = False
        self.screen = Screen(*size)
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            os.chdir(case.project)
            if stdout_file:
                fd = os.open(stdout_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                os.dup2(fd, 1)
                os.close(fd)
            os.execve(BINARY, [BINARY, "config", "init", *args], case.env)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", *size, 0, 0))
        self.flags = termios.tcgetattr(self.master)[3]
        case.addCleanup(self.close)

    def read(self, timeout=0.03):
        if self.eof or not select.select([self.master], [], [], timeout)[0]:
            return False
        try:
            chunk = os.read(self.master, 65536)
        except OSError as error:
            if error.errno != errno.EIO:
                raise
            chunk = b""
        self.eof = not chunk
        self.data.extend(chunk)
        self.screen.feed(chunk)
        return bool(chunk)

    def expect(self, text):
        self.wait_for(lambda: text in "\n".join(self.screen.lines), text)

    def wait_for(self, predicate, description):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            self.read()
            if predicate():
                while self.read():
                    pass
                self.offset = len(self.data)
                return
            if self.eof:
                break
        self.case.fail("Waiting for " + description + "\n" + self.data.decode(errors="replace"))

    def selected(self):
        return next((line.lstrip()[2:].split("│", 1)[0].strip() for line in self.screen.lines if line.lstrip().startswith("> ")), "")

    def resize(self, rows, columns):
        self.screen = Screen(rows, columns)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)

    def answer(self, prompt, keys=ENTER):
        self.expect(prompt)
        os.write(self.master, keys)

    def finish(self):
        deadline = time.monotonic() + 10
        while not self.eof and time.monotonic() < deadline:
            self.read()
        self.case.assertTrue(self.eof, self.data.decode(errors="replace"))
        _, status = os.waitpid(self.pid, 0)
        self.pid = None
        self.case.assertEqual(os.waitstatus_to_exitcode(status), 0, self.data.decode(errors="replace"))
        mask = termios.ECHO | termios.ICANON | termios.ISIG
        self.case.assertEqual(termios.tcgetattr(self.master)[3] & mask, self.flags & mask)
        return self.data.decode(errors="replace")

    def close(self):
        if self.pid is not None:
            try:
                os.killpg(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.waitpid(self.pid, 0)
        os.close(self.master)


@unittest.skipUnless(BINARY, "run through cargo test or pass the leanck executable")
class ConfigWizardTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="leanck-config-terminal-")
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.project = self.root / "project"
        self.project.mkdir()
        (self.project / "lean-toolchain").touch()
        self.user = self.root / "config" / "leanck" / "config.toml"
        self.env = {**os.environ, "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
                    "XDG_RUNTIME_DIR": str(self.root / "runtime"), "XDG_CACHE_HOME": str(self.root / "cache"),
                    "TMPDIR": str(self.root / "tmp"), "TERM": "xterm-256color", "NO_COLOR": "1"}
        self.env.pop("LEANCK_CONFIG", None)

    def show(self, path=None):
        args = [BINARY, "config", "show", "--json"]
        if path:
            args += ["--config", str(path)]
        result = subprocess.run(args, cwd=self.project, env=self.env, capture_output=True, check=True)
        return json.loads(result.stdout)

    def formatting(self, terminal, width=b"72\r"):
        terminal.answer("Comment line width", width)
        terminal.answer("Spelling convention", b"b" + ENTER)
        terminal.answer("Normalize file headers?")
        terminal.answer("Default license text")
        terminal.answer("Header format")
        terminal.answer("Sort imports?")
        terminal.answer("Format Markdown headings?")
        terminal.answer("Attribute placement")

    def test_project_setup_validation_review_back_and_inheritance(self):
        self.user.parent.mkdir(parents=True)
        self.user.write_text("[normalize]\nspelling = 'british'\n[normalize.rules.frontmatter]\nlicense_text = 'Team license.'\nheader_template = '/- {authors}: {license} -/'\n[daemon]\nmax_documents = 42\n")
        terminal = Terminal(self, [])
        terminal.answer("Where should these settings apply?")
        terminal.answer("What would you like to customize?", CLEAR + b" " + ENTER)
        terminal.answer("Comment line width", b"0\r")
        terminal.answer("Enter a number between", b"\x7f72\r")
        terminal.answer("Spelling convention")
        terminal.answer("Normalize file headers?")
        terminal.answer("Default license text")
        terminal.answer("Header format")
        terminal.answer("Sort imports?")
        terminal.answer("Format Markdown headings?")
        terminal.answer("Attribute placement")
        terminal.answer("Save this configuration?", b"e" + ENTER)
        self.assertFalse((self.project / "leanck.toml").exists())
        terminal.answer("What would you like to customize?", CLEAR + ENTER)
        terminal.answer("Save this configuration?", b"s" + ENTER)
        transcript = terminal.finish()
        self.assertNotIn("\x1b[32m", transcript)
        text = (self.project / "leanck.toml").read_text()
        self.assertIn("width = 72", text)
        self.assertNotIn("[daemon]", text)
        self.assertNotIn("[instances]", text)
        self.assertNotIn("[updates]", text)
        report = self.show()
        self.assertEqual(report["settings"]["normalize"]["spelling"], "british")
        frontmatter = report["settings"]["normalize"]["rules"]["frontmatter"]
        self.assertEqual(frontmatter["license_text"], "Team license.")
        self.assertEqual(frontmatter["header_template"], "/- {authors}: {license} -/")
        self.assertEqual(report["sources"]["normalize.rules.frontmatter.license_text"], str(self.user))
        self.assertEqual(report["sources"]["normalize.rules.frontmatter.header_template"], str(self.user))
        self.assertEqual(report["settings"]["daemon"]["max_documents"], 42)

    def test_license_and_header_format_validate_preserve_and_save(self):
        license_text = "Released under Apache 2.0 license as described in the file LICENSE."
        terminal = Terminal(self, ["leanck.toml"])
        terminal.answer("What would you like to customize?", CLEAR + b" " + ENTER)
        terminal.answer("Comment line width")
        terminal.answer("Spelling convention")
        terminal.answer("Normalize file headers?")
        terminal.expect("Default license text")
        terminal.expect("Released under Apache 2.0 license")
        terminal.expect("LICENSE.")
        terminal.answer("Default license text", b"bad -/" + NEXT)
        terminal.expect("license_text must not contain Lean comment delimiters")
        terminal.answer("Default license text", b"\x15" + license_text.encode() + NEXT)
        terminal.expect("Header format")
        terminal.expect(r"/-\n{copyright}\n{license}\nAuthors: {authors}\n-/")
        terminal.answer("Header format", br"/-\n{unknown}\n-/" + NEXT)
        terminal.expect("unknown placeholder {unknown}")
        template = r"/-\n{copyright}\nAuthors: {authors}\n{license}\n-/"
        terminal.answer("Header format", b"\x15" + template.encode() + NEXT)
        terminal.answer("Sort imports?", PREVIOUS)
        terminal.expect(template)
        terminal.answer("Header format", PREVIOUS)
        terminal.expect("Default license text")
        terminal.answer("Default license text", NEXT)
        terminal.answer("Header format", NEXT)
        terminal.answer("Sort imports?")
        terminal.answer("Format Markdown headings?")
        terminal.answer("Attribute placement")
        terminal.answer("Save this configuration?", b"s" + ENTER)
        terminal.finish()
        frontmatter = self.show()["settings"]["normalize"]["rules"]["frontmatter"]
        self.assertEqual(frontmatter["license_text"], license_text)
        self.assertEqual(frontmatter["header_template"], template.replace(r"\n", "\n"))

    def test_existing_hidden_file_preserves_comments_and_custom_settings(self):
        path = self.project / ".leanck.toml"
        original = "# Team conventions\n[normalize]\nwidth = 80 # columns\n[daemon]\nmax_documents = 42\n"
        path.write_text(original)
        terminal = Terminal(self, [])
        terminal.answer("Where should these settings apply?")
        terminal.answer("What would you like to customize?", CLEAR + b" " + ENTER)
        self.formatting(terminal)
        terminal.answer("Save this configuration?", b"s" + ENTER)
        terminal.finish()
        text = path.read_text()
        self.assertIn("width = 72 # columns", text)
        self.assertIn("# Team conventions", text)
        self.assertIn("max_documents = 42", text)
        self.assertFalse((self.project / "leanck.toml").exists())

    def test_user_setup_all_topics_and_redirected_stdout(self):
        stdout = self.root / "stdout"
        terminal = Terminal(self, ["--user"], stdout)
        terminal.answer("What would you like to customize?", ALL + ENTER)
        self.formatting(terminal, ENTER)
        terminal.answer("Diagnostics per page", b"12\r")
        terminal.answer("Which diagnostic severities?", ALL + ENTER)
        terminal.answer("Include build warnings?", b"y\r")
        terminal.answer("Fail builds on warnings?", b"y\r")
        terminal.answer("Build timeout (minutes)", b"0.5\r")
        terminal.answer("Loogle endpoints", b"\x7f" * 100 + b"https://example.test/json\r")
        terminal.answer("Search results per query", b"20\r")
        terminal.answer("Shut down idle daemons after (minutes)", b"0\r")
        terminal.answer("Concurrent elaboration requests per daemon", b"4\r")
        terminal.answer("Fetch the Mathlib cache before daemon startup?", b"n\r")
        terminal.answer("Maximum running daemons on this machine", b"3\r")
        terminal.answer("When the daemon limit is reached", b"r" + ENTER)
        terminal.answer("Check for new leanck releases?", b"n\r")
        terminal.answer("Install available releases automatically?")
        terminal.answer("Save this configuration?", b"s" + ENTER)
        terminal.finish()
        self.assertTrue(stdout.read_text().startswith("created "))
        self.assertNotIn("?", stdout.read_text())
        settings = self.show()["settings"]
        self.assertEqual(settings["instances"], {"limit": 3, "on_limit": "refuse"})
        self.assertEqual(settings["build"]["timeout_ms"], 30000)
        self.assertEqual(settings["diagnostics"]["limit"], 12)
        self.assertEqual(len(settings["diagnostics"]["severity"]), 4)
        self.assertEqual(settings["loogle"]["endpoints"], ["https://example.test/json"])
        self.assertEqual(settings["daemon"]["idle_timeout_ms"], 0)
        self.assertFalse(settings["updates"]["check"])

    def test_cancel_and_ctrl_c_do_not_write_or_create_directories(self):
        for key in [ENTER, b"\x03", b"\x1b"]:
            terminal = Terminal(self, ["--user"])
            terminal.answer("What would you like to customize?", CLEAR + ENTER)
            terminal.answer("Save this configuration?", key)
            if key == b"\x1b":
                terminal.answer("Cancel setup?", DOWN + ENTER)
            self.assertIn("Setup canceled", terminal.finish())
            self.assertFalse(self.user.parent.exists())

    def test_automation_flags_and_dumb_term_skip_the_wizard(self):
        for args in [["--json"], ["--no-interactive"], []]:
            env = dict(self.env)
            if not args:
                env["TERM"] = "dumb"
            self.env, previous = env, self.env
            path = self.project / "leanck.toml"
            terminal = Terminal(self, args)
            transcript = terminal.finish()
            self.env = previous
            self.assertNotIn("What would you like", transcript)
            if args == ["--json"]:
                self.assertIn('"created"', transcript)
            self.assertTrue(path.exists())
            path.unlink()

    def test_user_scope_can_be_selected_without_a_flag(self):
        terminal = Terminal(self, [])
        terminal.answer("Where should these settings apply?", DOWN + ENTER)
        terminal.answer("What would you like to customize?", CLEAR + ENTER)
        terminal.answer("Save this configuration?", UP + UP + ENTER)
        terminal.finish()
        self.assertTrue(self.user.exists())
        self.assertFalse((self.project / "leanck.toml").exists())

    def test_examples_appear_on_the_right_and_follow_terminal_resize(self):
        terminal = Terminal(self, ["leanck.toml"])
        terminal.answer("What would you like to customize?", CLEAR + b" " + ENTER)
        terminal.expect("Comment line width")
        self.assertTrue(any(line.find("Examples") > 60 for line in terminal.screen.lines))
        terminal.resize(24, 80)
        terminal.expect("Comment line width")
        self.assertNotIn("Examples", "\n".join(terminal.screen.lines))
        terminal.resize(40, 140)
        terminal.expect("Examples")
        terminal.answer("Comment line width")
        terminal.expect("Spelling convention")
        self.assertTrue(any(line.find("The colour of the centre") > 60 for line in terminal.screen.lines))
        os.write(terminal.master, b"\x03")
        terminal.finish()
        self.assertFalse((self.project / "leanck.toml").exists())

    def test_menus_have_no_filter_arrows_choose_and_typing_replaces_the_default(self):
        terminal = Terminal(self, [])
        terminal.expect("Where should these settings apply?")
        self.assertNotIn("filter", "\n".join(terminal.screen.lines).lower())
        terminal.answer("Where should these settings apply?", b" ")
        terminal.wait_for(lambda: terminal.selected() == "This project", "whitespace ignored")
        os.write(terminal.master, UP)
        terminal.wait_for(lambda: terminal.selected() == "All projects (user defaults)", "up arrow")
        os.write(terminal.master, DOWN)
        terminal.wait_for(lambda: terminal.selected() == "This project", "down arrow")
        os.write(terminal.master, ENTER)
        terminal.answer("What would you like to customize?", CLEAR + b" " + ENTER)
        terminal.expect("Comment line width")
        terminal.expect("│100")
        terminal.answer("Comment line width", b"88\r")
        terminal.expect("Spelling convention")
        self.assertNotIn("filter", "\n".join(terminal.screen.lines).lower())
        terminal.answer("Spelling convention", b" " + DOWN + ENTER)
        terminal.expect("Normalize file headers?")
        self.assertNotIn("filter", "\n".join(terminal.screen.lines).lower())
        terminal.answer("Normalize file headers?")
        terminal.answer("Default license text")
        terminal.answer("Header format")
        terminal.answer("Sort imports?")
        terminal.answer("Format Markdown headings?")
        terminal.answer("Attribute placement")
        terminal.answer("Save this configuration?", b"s\r")
        terminal.finish()
        settings = self.show()["settings"]
        self.assertEqual(settings["normalize"]["width"], 88)
        self.assertEqual(settings["normalize"]["spelling"], "british")

    def test_escape_confirmation_preserves_the_current_field_and_draft(self):
        terminal = Terminal(self, ["leanck.toml"])
        terminal.answer("What would you like to customize?", CLEAR + b" " + ENTER)
        terminal.answer("Comment line width", b"72\x1b")
        terminal.expect("Cancel setup?")
        self.assertEqual(terminal.selected(), "Keep editing")
        terminal.answer("Cancel setup?")
        terminal.expect("Comment line width")
        terminal.expect("│72")
        terminal.answer("Comment line width", b"\x1b")
        terminal.answer("Cancel setup?", b"\x1b")
        terminal.expect("Comment line width")
        terminal.expect("│72")
        terminal.answer("Comment line width")
        terminal.answer("Spelling convention", b"\x1b")
        terminal.answer("Cancel setup?", DOWN + ENTER)
        self.assertIn("Setup canceled", terminal.finish())
        self.assertFalse((self.project / "leanck.toml").exists())

    def test_text_arrows_edit_and_tab_focuses_navigation_buttons(self):
        terminal = Terminal(self, ["leanck.toml"])
        terminal.answer("What would you like to customize?", CLEAR + b" " + ROOT_NEXT)
        terminal.expect("Comment line width")
        terminal.expect("Previous")
        terminal.expect("Next")
        terminal.answer("Comment line width", LEFT + b"2")
        terminal.expect("│1020 ")
        terminal.answer("Comment line width", RIGHT + b"\x7f")
        terminal.expect("│102 ")
        terminal.answer("Comment line width", b"\x1572\t")
        terminal.expect("> Previous <")
        terminal.answer("Comment line width", RIGHT)
        terminal.expect("> Next <")
        terminal.expect("Comment line width")
        terminal.answer("Comment line width", ENTER)
        terminal.answer("Spelling convention", DOWN + PREVIOUS)
        terminal.expect("Comment line width")
        terminal.expect("│72 ")
        # Shift-Tab from the field focuses Next; neither this nor arrows submit.
        terminal.answer("Comment line width", b"\x1b[Z")
        terminal.expect("> Next <")
        terminal.answer("Comment line width", ENTER)
        terminal.expect("Spelling convention")
        self.assertEqual(terminal.selected(), "British English")
        os.write(terminal.master, b"\x03")
        terminal.finish()
        self.assertFalse((self.project / "leanck.toml").exists())

    def test_buttons_navigate_screens_and_preserve_all_field_types(self):
        terminal = Terminal(self, ["leanck.toml"])
        terminal.answer("What would you like to customize?", ROOT_NEXT)
        terminal.answer("Comment line width", b"0" + NEXT)
        terminal.expect("Enter a number between")
        terminal.answer("Comment line width", PREVIOUS)
        terminal.answer("What would you like to customize?", ROOT_NEXT)
        terminal.expect("Comment line width")
        terminal.expect("│0")
        terminal.answer("Comment line width", b"\x1572" + NEXT)
        terminal.answer("Spelling convention", DOWN + PREVIOUS)
        terminal.expect("Comment line width")
        terminal.expect("│72")
        terminal.answer("Comment line width", b"80" + NEXT)
        terminal.expect("Spelling convention")
        self.assertEqual(terminal.selected(), "British English")
        terminal.answer("Spelling convention", NEXT)
        terminal.answer("Normalize file headers?", DOWN + NEXT)
        terminal.answer("Default license text", NEXT)
        terminal.answer("Header format", NEXT)
        terminal.answer("Sort imports?", PREVIOUS)
        terminal.answer("Header format", PREVIOUS)
        terminal.answer("Default license text", PREVIOUS)
        terminal.expect("Normalize file headers?")
        self.assertEqual(terminal.selected(), "No")
        terminal.answer("Normalize file headers?", NEXT)
        terminal.answer("Default license text", NEXT)
        terminal.answer("Header format", NEXT)
        terminal.answer("Sort imports?", NEXT)
        terminal.answer("Format Markdown headings?", NEXT)
        terminal.answer("Attribute placement", DOWN + NEXT)
        terminal.answer("Diagnostics per page", b"9" + PREVIOUS)
        terminal.expect("Attribute placement")
        self.assertEqual(terminal.selected(), "Above the declaration")
        terminal.answer("Attribute placement", NEXT)
        terminal.expect("Diagnostics per page")
        terminal.expect("│9")
        terminal.answer("Diagnostics per page", NEXT)
        terminal.answer("Which diagnostic severities?", DOWN + b" " + NEXT)
        terminal.answer("Include build warnings?", PREVIOUS)
        terminal.expect("Which diagnostic severities?")
        self.assertIn("[x] warning", "\n".join(terminal.screen.lines))
        terminal.answer("Which diagnostic severities?", NEXT)
        terminal.answer("Include build warnings?", DOWN + NEXT)
        terminal.answer("Fail builds on warnings?", NEXT)
        terminal.answer("Build timeout (minutes)", NEXT)
        terminal.answer("Save this configuration?", PREVIOUS)
        terminal.answer("Build timeout (minutes)", b"0.5" + NEXT)
        terminal.answer("Save this configuration?", RIGHT)
        terminal.expect("Save this configuration?")
        self.assertFalse((self.project / "leanck.toml").exists())
        terminal.answer("Save this configuration?", b"s" + ENTER)
        terminal.finish()
        settings = self.show()["settings"]
        self.assertEqual(settings["normalize"]["width"], 80)
        self.assertEqual(settings["normalize"]["spelling"], "british")
        self.assertFalse(settings["normalize"]["header"])
        self.assertEqual(settings["normalize"]["rules"]["attributes"]["placement"], "above")
        self.assertEqual(settings["diagnostics"]["limit"], 9)
        self.assertEqual(settings["diagnostics"]["severity"], ["error", "warning"])
        self.assertTrue(settings["build"]["warnings"])
        self.assertEqual(settings["build"]["timeout_ms"], 30000)

    def test_back_to_scope_keeps_project_and_user_drafts_separate(self):
        terminal = Terminal(self, [])
        terminal.answer("Where should these settings apply?", ROOT_NEXT)
        terminal.answer("What would you like to customize?", CLEAR + b" " + NEXT)
        terminal.answer("Comment line width", b"72" + NEXT)
        terminal.answer("Spelling convention", b"b" + NEXT)
        terminal.answer("Normalize file headers?", PREVIOUS)
        terminal.answer("Spelling convention", PREVIOUS)
        terminal.answer("Comment line width", PREVIOUS)
        terminal.answer("What would you like to customize?", PREVIOUS)
        terminal.answer("Where should these settings apply?", DOWN + ROOT_NEXT)
        terminal.answer("What would you like to customize?", CLEAR + b" " + NEXT)
        terminal.expect("Comment line width")
        terminal.expect("│100")
        terminal.answer("Comment line width", b"90" + NEXT)
        terminal.expect("Spelling convention")
        self.assertEqual(terminal.selected(), "American English")
        terminal.answer("Spelling convention", PREVIOUS)
        terminal.answer("Comment line width", PREVIOUS)
        terminal.answer("What would you like to customize?", PREVIOUS)
        terminal.answer("Where should these settings apply?", UP + ROOT_NEXT)
        terminal.answer("What would you like to customize?", NEXT)
        terminal.expect("Comment line width")
        terminal.expect("│72")
        terminal.answer("Comment line width", NEXT)
        terminal.expect("Spelling convention")
        self.assertEqual(terminal.selected(), "British English")
        os.write(terminal.master, b"\x03")
        terminal.finish()
        self.assertFalse((self.project / "leanck.toml").exists())
        self.assertFalse(self.user.exists())


if __name__ == "__main__":
    unittest.main()
