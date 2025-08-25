use anyhow::{Context, Result};
use std::io::{self, Write};
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, info, warn};

/// Clear the tweet cache (removes all downloaded tweets and media)
pub async fn execute(output_dir: &PathBuf, force: bool) -> Result<()> {
    if !force {
        // Ask for confirmation
        print!(
            "Are you sure you want to delete all cached tweets and media from {path}? [y/N] ",
            path = output_dir.display()
        );
        io::stdout().flush().context("Failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("Failed to read user input")?;

        if !input.trim().eq_ignore_ascii_case("y") {
            info!("Cache deletion cancelled");
            return Ok(());
        }
    }

    // Get list of files
    let mut entries = fs::read_dir(output_dir)
        .await
        .context("Failed to read output directory")?;

    let mut deleted_count = 0;

    while let Some(entry) = entries
        .next_entry()
        .await
        .context("Failed to read directory entry")?
    {
        let path = entry.path();
        if path.is_file() {
            if let Err(e) = fs::remove_file(&path).await {
                warn!("Failed to delete {path}: {e}", path = path.display());
            } else {
                debug!("Deleted {path}", path = path.display());
                deleted_count += 1;
            }
        }
    }

    info!("Deleted {deleted_count} files from the cache");
    Ok(())
}
