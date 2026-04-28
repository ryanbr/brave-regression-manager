use anyhow::{anyhow, Result};
use std::path::PathBuf;

use crate::cli::ProfileCmd;
use crate::paths;

pub mod reset;
pub mod seed;

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub dir:  PathBuf,
}

pub fn list() -> Result<Vec<Profile>> {
    let dir = paths::profiles_dir();
    if !dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            out.push(Profile {
                name: entry.file_name().to_string_lossy().into_owned(),
                dir:  entry.path(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn create(name: &str) -> Result<Profile> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(anyhow!("invalid profile name: {name}"));
    }
    let dir = paths::profile_dir(name);
    std::fs::create_dir_all(&dir)?;
    Ok(Profile { name: name.into(), dir })
}

pub fn delete(name: &str) -> Result<()> {
    let dir = paths::profile_dir(name);
    if !dir.exists() { return Err(anyhow!("no such profile: {name}")); }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

pub async fn handle(cmd: ProfileCmd) -> Result<()> {
    paths::ensure_dirs()?;
    match cmd {
        ProfileCmd::New    { name } => { create(&name)?; println!("created profile {name}"); Ok(()) }
        ProfileCmd::Delete { name } => { delete(&name)?; println!("deleted profile {name}"); Ok(()) }
        ProfileCmd::List   => {
            for p in list()? { println!("{}\t{}", p.name, p.dir.display()); }
            Ok(())
        }
        ProfileCmd::Reset  { name, scope } => {
            let scope = reset::ResetScope::parse(&scope)?;
            reset::reset_profile(&paths::profile_dir(&name), scope)?;
            println!("reset {name} ({:?})", scope);
            Ok(())
        }
        ProfileCmd::Seed   { name, version } => {
            seed::seed_lists(&name, &version).await?;
            println!("seeded lists for profile {name} using {version}");
            Ok(())
        }
    }
}
