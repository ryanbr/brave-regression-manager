use anyhow::Result;
use std::path::Path;

use crate::diff::CellDiff;

pub fn write_html(path: &Path, diffs: &[CellDiff]) -> Result<()> {
    let mut html = String::from("<html><body><table border=1>");
    html.push_str("<tr><th>URL</th><th>Newly blocked</th><th>Newly allowed</th><th>New errors</th></tr>");
    for d in diffs {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&d.url),
            d.newly_blocked.iter().cloned().collect::<Vec<_>>().join("<br>"),
            d.newly_allowed.iter().cloned().collect::<Vec<_>>().join("<br>"),
            d.new_console_errors.join("<br>"),
        ));
    }
    html.push_str("</table></body></html>");
    std::fs::write(path, html)?;
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
