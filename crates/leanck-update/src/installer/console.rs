//! Installer messages on stderr: `[INFO]`, `[WARN]`, and ` [ERR]:` lines.
use std::io::Write;

/// ANSI styles, all empty when color is off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: &'static str,
    pub cyan: &'static str,
    pub green: &'static str,
    pub yellow: &'static str,
    pub red: &'static str,
    pub reset: &'static str,
}
impl Style {
    pub const PLAIN: Style = Style {
        bold: "",
        cyan: "",
        green: "",
        yellow: "",
        red: "",
        reset: "",
    };
    pub const COLOR: Style = Style {
        bold: "\x1b[1m",
        cyan: "\x1b[36m",
        green: "\x1b[32m",
        yellow: "\x1b[33m",
        red: "\x1b[31m",
        reset: "\x1b[0m",
    };
    /// Color only for a terminal, without `--no-color`, a non-empty
    /// `NO_COLOR`, or `TERM=dumb`.
    pub fn detect(no_color: bool, terminal: bool) -> Style {
        let disabled = no_color
            || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
            || std::env::var_os("TERM").is_some_and(|t| t == "dumb");
        if terminal && !disabled {
            Style::COLOR
        } else {
            Style::PLAIN
        }
    }
}

/// Writes prefixed installer messages.
pub struct Console<W: Write> {
    pub style: Style,
    pub quiet: bool,
    out: W,
}
impl Console<std::io::Stderr> {
    pub fn stderr(style: Style, quiet: bool) -> Self {
        Self::new(std::io::stderr(), style, quiet)
    }
}
impl<W: Write> Console<W> {
    pub fn new(out: W, style: Style, quiet: bool) -> Self {
        Self { style, quiet, out }
    }
    /// `[INFO] MESSAGE`, unless quiet.
    pub fn info(&mut self, message: &str) {
        if !self.quiet {
            let s = self.style;
            let _ = writeln!(self.out, "{}[INFO]{} {message}", s.cyan, s.reset);
        }
    }
    /// `[WARN] MESSAGE`, always.
    pub fn warn(&mut self, message: &str) {
        let s = self.style;
        let _ = writeln!(self.out, "{}[WARN]{} {message}", s.yellow, s.reset);
    }
    /// ` [ERR]: MESSAGE`, always.
    pub fn error(&mut self, message: &str) {
        let s = self.style;
        let _ = writeln!(self.out, "{} [ERR]:{} {message}", s.red, s.reset);
    }
    /// Unprefixed text, always.
    pub fn raw(&mut self, text: &str) {
        let _ = self.out.write_all(text.as_bytes());
        let _ = self.out.flush();
    }
    pub fn into_inner(self) -> W {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_match_the_shell_installer_and_quiet_drops_only_info() {
        let mut console = Console::new(vec![], Style::PLAIN, true);
        console.info("hidden");
        console.warn("careful");
        console.error("failed");
        assert_eq!(
            String::from_utf8(console.into_inner()).unwrap(),
            "[WARN] careful\n [ERR]: failed\n"
        );
        let mut console = Console::new(vec![], Style::COLOR, false);
        console.info("shown");
        assert_eq!(
            String::from_utf8(console.into_inner()).unwrap(),
            "\x1b[36m[INFO]\x1b[0m shown\n"
        );
    }
}
