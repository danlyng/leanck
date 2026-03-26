//! JSON output matching Python's `json.dumps(value, indent=2) + "\n"`, which
//! earlier packages were written with: two-space indentation, original key
//! order, and non-ASCII characters escaped.

use std::path::Path;

use anyhow::Result;

pub(crate) fn to_string(value: &serde_json::Value) -> Result<String> {
    let pretty = serde_json::to_string_pretty(value)?;
    // Outside strings, serialized JSON is ASCII, so this escapes only string contents.
    let mut out = String::with_capacity(pretty.len() + 1);
    for c in pretty.chars() {
        if (' '..='~').contains(&c) || c == '\n' {
            out.push(c);
        } else {
            let mut units = [0; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out.push('\n');
    Ok(out)
}

pub(crate) fn write(path: &Path, value: &serde_json::Value) -> Result<()> {
    crate::write(path, to_string(value)?)
}
