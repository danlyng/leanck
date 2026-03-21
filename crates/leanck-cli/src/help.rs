//! Shared presentation for command help.
pub(crate) fn examples(body: &str) -> String {
    format!(
        "{}\n{body}",
        color_print::cstr!("<underline,bold>Examples:</>")
    )
}
