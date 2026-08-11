use std::path::Path;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, bounded};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::repository::Repository;

/// Coalesces filesystem notifications into a wake-up signal for the UI.
///
/// The watcher itself deliberately does not decide which events matter. Editors
/// commonly replace files through temporary renames, and Git can update HEAD,
/// the index, and refs in several different ways. Debouncing is handled by the
/// app before it asks Git for a fresh snapshot.
pub struct RepositoryWatcher {
    _watcher: RecommendedWatcher,
    pub events: Receiver<()>,
}

impl RepositoryWatcher {
    pub fn start(repo: &Repository) -> Result<Self> {
        let (sender, events) = bounded(1);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    // A full channel already represents a pending refresh.
                    let _ = sender.try_send(());
                }
            })
            .context("create repository filesystem watcher")?;

        watcher
            .watch(&repo.root, RecursiveMode::Recursive)
            .with_context(|| format!("watch repository {}", repo.root.display()))?;

        let git_dir = repo.git_dir()?;
        if !is_inside(&git_dir, &repo.root) {
            watcher
                .watch(&git_dir, RecursiveMode::Recursive)
                .with_context(|| format!("watch Git directory {}", git_dir.display()))?;
        }

        Ok(Self {
            _watcher: watcher,
            events,
        })
    }
}

fn is_inside(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}
