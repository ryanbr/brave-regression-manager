use anyhow::Result;

/// Offline path: feed lists directly into `adblock-rust` and check whether
/// each URL would be blocked, without spinning up a browser.
/// Stub — wires up to the `adblock` crate next.
pub fn check(_list_text: &str, _url: &str, _source_url: &str) -> Result<bool> {
    Ok(false)
}
