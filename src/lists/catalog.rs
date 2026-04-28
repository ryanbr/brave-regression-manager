use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CatalogEntry {
    pub uuid:         String,
    pub component_id: String,
    pub title:        String,
    #[serde(default)] pub url: String,
}

pub type Catalog = HashMap<String, CatalogEntry>;

/// Parse Brave's regional adblock catalog. Brave ships either `list_catalog.json`
/// or a similarly-named manifest under the catalog component folder.
pub fn load(component_version_dir: &Path) -> Result<Catalog> {
    for name in ["list_catalog.json", "regional_catalog.json", "catalog.json"] {
        let p = component_version_dir.join(name);
        if p.exists() {
            let s = std::fs::read_to_string(&p)?;
            let entries: Vec<CatalogEntry> = serde_json::from_str(&s).unwrap_or_default();
            return Ok(entries.into_iter().map(|e| (e.uuid.clone(), e)).collect());
        }
    }
    Ok(HashMap::new())
}
