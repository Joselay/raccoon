use std::{ffi::OsString, num::NonZeroUsize, path::PathBuf};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, bounded};
use lru::LruCache;

use crate::{
    cli::LaunchTarget,
    dashboard::{self, DashboardData},
    diff::DiffDocument,
    git_diff,
    highlight::{HighlightedDiff, Highlighter},
    repository::Repository,
};

const COMMAND_CAPACITY: usize = 8;
const DIFF_CACHE_CAPACITY: usize = 32;
const HIGHLIGHT_CACHE_CAPACITY: usize = 16;

pub enum GitCommand {
    Dashboard {
        request_id: u64,
        history_path: Option<PathBuf>,
    },
    Diff {
        request_id: u64,
        target: LaunchTarget,
    },
    CommitFiles {
        request_id: u64,
        revision: OsString,
        path: Option<PathBuf>,
    },
    DiscardWorkingTree {
        request_id: u64,
    },
}

pub enum GitPayload {
    Dashboard(DashboardData),
    Diff(DiffDocument),
    CommitFiles {
        revision: OsString,
        files: Vec<dashboard::ChangeEntry>,
    },
    DiscardedWorkingTree,
}

pub struct GitResponse {
    pub request_id: u64,
    pub result: Result<GitPayload>,
}

pub struct GitWorker {
    commands: Sender<GitCommand>,
    pub responses: Receiver<GitResponse>,
}

impl GitWorker {
    pub fn start(repo: Repository) -> Self {
        let (commands, command_receiver) = bounded(COMMAND_CAPACITY);
        let (response_sender, responses) = bounded(COMMAND_CAPACITY);
        std::thread::Builder::new()
            .name("raccoon-git-worker".into())
            .spawn(move || git_loop(repo, command_receiver, response_sender))
            .expect("spawn Git worker");
        Self {
            commands,
            responses,
        }
    }

    pub fn request(&self, command: GitCommand) -> Result<()> {
        self.commands
            .try_send(command)
            .context("Git worker command queue is full")
    }
}

fn git_loop(repo: Repository, commands: Receiver<GitCommand>, responses: Sender<GitResponse>) {
    let mut cache = LruCache::new(NonZeroUsize::new(DIFF_CACHE_CAPACITY).unwrap());
    while let Ok(command) = commands.recv() {
        let (request_id, result) = match command {
            GitCommand::Dashboard {
                request_id,
                history_path,
            } => (
                request_id,
                dashboard::load(&repo, history_path.as_ref()).map(GitPayload::Dashboard),
            ),
            GitCommand::Diff { request_id, target } => {
                let result = load_cached_diff(&repo, &target, &mut cache).map(GitPayload::Diff);
                (request_id, result)
            }
            GitCommand::CommitFiles {
                request_id,
                revision,
                path,
            } => {
                let result =
                    dashboard::load_commit_files(&repo, &revision, path.as_ref()).map(|files| {
                        GitPayload::CommitFiles {
                            revision: revision.clone(),
                            files,
                        }
                    });
                (request_id, result)
            }
            GitCommand::DiscardWorkingTree { request_id } => {
                let output = repo.git(["restore", "--worktree", "--", "."]);
                let result = output.and_then(|output| {
                    if output.status.success() {
                        Ok(GitPayload::DiscardedWorkingTree)
                    } else {
                        anyhow::bail!(
                            "discard working-tree changes: {}",
                            String::from_utf8_lossy(&output.stderr).trim()
                        )
                    }
                });
                (request_id, result)
            }
        };
        if responses.send(GitResponse { request_id, result }).is_err() {
            break;
        }
    }
}

fn load_cached_diff(
    repo: &Repository,
    target: &LaunchTarget,
    cache: &mut LruCache<ResolvedDiffKey, DiffDocument>,
) -> Result<DiffDocument> {
    let Some(key) = resolved_key(repo, target)? else {
        return git_diff::load(repo, target);
    };
    if let Some(document) = cache.get(&key) {
        return Ok(document.clone());
    }
    let document = git_diff::load(repo, target)?;
    cache.put(key, document.clone());
    Ok(document)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ResolvedDiffKey {
    target: LaunchTarget,
    resolved_revisions: Vec<u8>,
}

fn resolved_key(repo: &Repository, target: &LaunchTarget) -> Result<Option<ResolvedDiffKey>> {
    let revisions: Vec<&std::ffi::OsStr> = match target {
        LaunchTarget::Commit { revision, .. } | LaunchTarget::Show { revision, .. } => {
            vec![revision.as_os_str()]
        }
        LaunchTarget::Compare { left, right, .. } => vec![left.as_os_str(), right.as_os_str()],
        _ => return Ok(None),
    };
    let mut resolved = Vec::new();
    for revision in revisions {
        let args = vec![
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from("--end-of-options"),
            revision.to_owned(),
        ];
        let output = repo.git(args)?;
        if !output.status.success() {
            return Ok(None);
        }
        resolved.extend_from_slice(&output.stdout);
    }
    Ok(Some(ResolvedDiffKey {
        target: target.clone(),
        resolved_revisions: resolved,
    }))
}

pub struct HighlightCommand {
    pub request_id: u64,
    pub document: DiffDocument,
}

pub struct HighlightResponse {
    pub request_id: u64,
    pub result: Result<HighlightedDiff>,
}

pub struct HighlightWorker {
    commands: Sender<HighlightCommand>,
    pub responses: Receiver<HighlightResponse>,
}

impl HighlightWorker {
    pub fn start() -> Self {
        let (commands, command_receiver) = bounded(COMMAND_CAPACITY);
        let (response_sender, responses) = bounded(COMMAND_CAPACITY);
        std::thread::Builder::new()
            .name("raccoon-highlight-worker".into())
            .spawn(move || highlight_loop(command_receiver, response_sender))
            .expect("spawn highlighting worker");
        Self {
            commands,
            responses,
        }
    }

    pub fn request(&self, command: HighlightCommand) -> Result<()> {
        self.commands
            .try_send(command)
            .context("highlighting worker command queue is full")
    }
}

fn highlight_loop(commands: Receiver<HighlightCommand>, responses: Sender<HighlightResponse>) {
    let highlighter = Highlighter::new();
    let mut cache: LruCache<u64, HighlightedDiff> =
        LruCache::new(NonZeroUsize::new(HIGHLIGHT_CACHE_CAPACITY).unwrap());
    while let Ok(command) = commands.recv() {
        let fingerprint = command.document.fingerprint();
        let result = if let Some(highlighted) = cache.get(&fingerprint) {
            Ok(highlighted.clone())
        } else {
            highlighter
                .highlight(&command.document)
                .inspect(|highlighted| {
                    cache.put(fingerprint, highlighted.clone());
                })
        };
        if responses
            .send(HighlightResponse {
                request_id: command.request_id,
                result,
            })
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, process::Command, time::Duration};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn discard_restores_index_and_keeps_staged_and_untracked_content() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Raccoon Test"]);
        git(&["config", "user.email", "raccoon@example.test"]);
        fs::write(root.join("tracked.txt"), "committed\n").unwrap();
        git(&["add", "tracked.txt"]);
        git(&["commit", "-qm", "initial"]);

        fs::write(root.join("tracked.txt"), "staged\n").unwrap();
        git(&["add", "tracked.txt"]);
        fs::write(root.join("tracked.txt"), "working tree\n").unwrap();
        fs::write(root.join("untracked.txt"), "keep me\n").unwrap();

        let worker = GitWorker::start(Repository::discover(root).unwrap());
        worker
            .request(GitCommand::DiscardWorkingTree { request_id: 7 })
            .unwrap();
        let response = worker
            .responses
            .recv_timeout(Duration::from_secs(5))
            .unwrap();

        assert_eq!(response.request_id, 7);
        assert!(matches!(
            response.result.unwrap(),
            GitPayload::DiscardedWorkingTree
        ));
        assert_eq!(
            fs::read_to_string(root.join("tracked.txt")).unwrap(),
            "staged\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("untracked.txt")).unwrap(),
            "keep me\n"
        );
        let staged = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(root)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&staged.stdout).trim(),
            "tracked.txt"
        );
    }
}
