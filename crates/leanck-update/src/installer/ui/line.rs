//! The line-by-line installer dialog. Questions are printed without redrawing;
//! replies are read one line at a time. Menus accept numbers or value words.
use super::flow::{Effect, Flow, Output, Question};
use crate::installer::{Answers, Plan, PlanFailure, Settings, Style};
use std::io::{self, BufRead, Write};

fn write_prompt(out: &mut impl Write, style: Style, label: &str, default: &str) -> io::Result<()> {
    write!(
        out,
        "{}{}{label}{} [{default}]: ",
        style.bold, style.cyan, style.reset
    )?;
    out.flush()
}

fn write_output(out: &mut impl Write, style: Style, output: Vec<Output>) -> io::Result<()> {
    for entry in output {
        match entry {
            // The terminal already shows what was typed.
            Output::Answer { .. } => {}
            Output::Warn(message) => {
                writeln!(out, "{}[WARN]{} {message}", style.yellow, style.reset)?
            }
            Output::Error(message) => {
                writeln!(out, "{} [ERR]:{} {message}", style.red, style.reset)?
            }
            Output::Text(text) => write!(out, "{text}")?,
        }
    }
    out.flush()
}

/// Read one reply; `None` at the end of input. Strips the line ending.
fn read_reply(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let reply = line.strip_suffix('\n').unwrap_or(&line);
    Ok(Some(reply.strip_suffix('\r').unwrap_or(reply).to_owned()))
}

/// Run `flow` to completion, planning through `plan`. Returns the final
/// effect: confirmed, cancelled, interrupted, or failed.
pub fn run(
    flow: &mut Flow,
    input: &mut impl BufRead,
    out: &mut impl Write,
    style: Style,
    mut plan: impl FnMut(&Settings, &Answers) -> Result<Plan, PlanFailure>,
) -> io::Result<Effect> {
    loop {
        write_output(out, style, flow.take_output())?;
        let mut effect = match flow.question() {
            Question::Menu {
                title,
                options,
                default,
            } => {
                writeln!(out, "{}{}{title}{}", style.bold, style.cyan, style.reset)?;
                for (index, option) in options.iter().enumerate() {
                    writeln!(out, "  {}) {}", index + 1, option.label)?;
                }
                write!(out, "Choice [{}]: ", default + 1)?;
                out.flush()?;
                match read_reply(input)? {
                    Some(reply) => match reply.parse::<usize>() {
                        Ok(number) if (1..=options.len()).contains(&number) => {
                            flow.choose(number - 1)
                        }
                        _ => flow.answer(&reply),
                    },
                    None => flow.interrupt(),
                }
            }
            Question::Prompt { label, default } => {
                write_prompt(out, style, &label, &default)?;
                match read_reply(input)? {
                    Some(reply) => flow.answer(&reply),
                    None => flow.interrupt(),
                }
            }
            // The plain flow never opens the form; close it unchanged.
            Question::Form => flow.form_closed(None),
            Question::Planning => Effect::Plan,
            Question::Done => Effect::Interrupted,
        };
        while effect == Effect::Plan {
            effect = flow.planned(plan(flow.settings(), flow.answers()));
        }
        if effect != Effect::None {
            write_output(out, style, flow.take_output())?;
            return Ok(effect);
        }
    }
}

/// Ask a yes/no question until answered; `None` at the end of input.
pub fn ask_yes_no(
    input: &mut impl BufRead,
    out: &mut impl Write,
    style: Style,
    label: &str,
    default: bool,
) -> io::Result<Option<bool>> {
    loop {
        write_prompt(out, style, label, if default { "yes" } else { "no" })?;
        let Some(reply) = read_reply(input)? else {
            return Ok(None);
        };
        match reply.as_str() {
            "" => return Ok(Some(default)),
            "y" | "Y" | "yes" | "YES" => return Ok(Some(true)),
            "n" | "N" | "no" | "NO" => return Ok(Some(false)),
            _ => writeln!(
                out,
                "{}[WARN]{} Enter yes or no.",
                style.yellow, style.reset
            )?,
        }
    }
}

/// Ask a yes/no question on the controlling terminal, never stdin (which
/// may be the piped installer script). `None` when the terminal closes.
pub fn terminal_yes_no(style: Style, label: &str, default: bool) -> io::Result<Option<bool>> {
    let tty = std::fs::File::open("/dev/tty")?;
    ask_yes_no(
        &mut io::BufReader::new(tty),
        &mut io::stderr(),
        style,
        label,
        default,
    )
}
