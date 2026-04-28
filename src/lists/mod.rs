use anyhow::Result;

use crate::cli::ListsCmd;
use crate::paths;

pub mod discover;
pub mod catalog;
pub mod pin;
pub mod guard;
pub mod watcher;
pub mod merge;
pub mod mutate;
pub mod apply;
pub mod retention;

pub async fn handle(cmd: ListsCmd) -> Result<()> {
    paths::ensure_dirs()?;
    match cmd {
        ListsCmd::Show { profile } => {
            let dir = paths::profile_dir(&profile);
            for l in discover::enabled_lists(&dir)? {
                println!("{}\t{}\t{}\t{:?}\tlines={}\t{}",
                    l.name, l.version, l.component_id,
                    l.kind, l.line_count,
                    l.path.display());
            }
            Ok(())
        }
        ListsCmd::Apply { profile, version } => {
            apply::apply_and_relaunch(&profile, &version).await
        }
        ListsCmd::Pin { profile, verify, unpin } => {
            let dir = paths::profile_dir(&profile);
            if unpin       { pin::unpin_all(&dir) }
            else if verify { pin::verify_all(&dir).map(|r| println!("{r:?}")) }
            else           { pin::pin_all(&dir).map(|n| println!("pinned {n} components")) }
        }
        ListsCmd::Update { profile, action } => {
            let dir = paths::profile_dir(&profile);
            match action.as_str() {
                "review"   => { for u in guard::pending_updates(&dir)? { println!("{u:?}"); } Ok(()) }
                "accept"   => { guard::accept_all(&dir, /*merge=*/true) }
                "overwrite"=> { guard::accept_all(&dir, /*merge=*/false) }
                _ => Err(anyhow::anyhow!("unknown lists update action: {action}")),
            }
        }
    }
}
