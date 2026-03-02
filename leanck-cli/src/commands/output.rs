//! Frontend output selection; live-query rendering lives in leanck-client.
pub(crate) fn print_json(body: &impl serde::Serialize) -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::to_value(body)?)?
    );
    Ok(())
}
