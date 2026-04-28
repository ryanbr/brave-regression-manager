use anyhow::Result;
use crate::cli::BisectCmd;

pub async fn handle(cmd: BisectCmd) -> Result<()> {
    match cmd {
        BisectCmd::Versions { good, bad, url, check } => {
            println!("[bisect-versions] good={good} bad={bad} url={url} check={check:?}");
            // Skeleton: walk Nightly tags between `good` and `bad`, install each,
            // run `url`, classify, narrow window, print final culprit tag.
            Ok(())
        }
        BisectCmd::Rules { version, list, url, expect } => {
            println!("[bisect-rules] version={version} list={list} url={url} expect={expect}");
            // Skeleton: binary-search list lines via `lists::mutate::ListBuffer`.
            Ok(())
        }
    }
}
