use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "raccoon",
    version,
    about = "A performance-first, read-only Git TUI",
    after_help = "EXAMPLES:\n  raccoon diff src/main.rs\n  raccoon diff --staged src/main.rs\n  raccoon diff --commit HEAD~1 src/main.rs\n  raccoon diff main feature -- src/main.rs"
)]
pub struct Cli {
    /// Repository or a path inside it
    #[arg(long, global = true, value_name = "PATH")]
    pub repo: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open a working-tree, staged, commit, or revision-comparison diff
    Diff(DiffArgs),
    /// Open a revision and its diff
    Show(RevisionArgs),
    /// Open commit history, optionally focused on a path
    History(OptionalPathArgs),
    /// Open with the branch panel focused
    Branches,
    /// Open with uncommitted changes focused
    Changes,
}

#[derive(Debug, Args)]
pub struct DiffArgs {
    /// Compare the index with HEAD
    #[arg(long, conflicts_with = "commit")]
    pub staged: bool,

    /// Show the change introduced by REV
    #[arg(long, value_name = "REV")]
    pub commit: Option<OsString>,

    /// PATH, REV-A REV-B, or REV-A REV-B [PATH]
    #[arg(value_name = "ARG", num_args = 0..=3, trailing_var_arg = true)]
    pub args: Vec<OsString>,
}

#[derive(Debug, Args)]
pub struct RevisionArgs {
    pub revision: OsString,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct OptionalPathArgs {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LaunchTarget {
    Dashboard,
    WorkingTree {
        path: PathBuf,
    },
    Staged {
        path: PathBuf,
    },
    Commit {
        revision: OsString,
        path: Option<PathBuf>,
    },
    Compare {
        left: OsString,
        right: OsString,
        path: Option<PathBuf>,
    },
    Show {
        revision: OsString,
        path: Option<PathBuf>,
    },
    History {
        path: Option<PathBuf>,
    },
    Branches,
    Changes,
}

impl Cli {
    pub fn launch_target(self) -> Result<LaunchTarget> {
        let Some(command) = self.command else {
            return Ok(LaunchTarget::Dashboard);
        };
        match command {
            Command::Diff(args) => args.launch_target(),
            Command::Show(args) => Ok(LaunchTarget::Show {
                revision: args.revision,
                path: args.path,
            }),
            Command::History(args) => Ok(LaunchTarget::History { path: args.path }),
            Command::Branches => Ok(LaunchTarget::Branches),
            Command::Changes => Ok(LaunchTarget::Changes),
        }
    }
}

impl DiffArgs {
    fn launch_target(self) -> Result<LaunchTarget> {
        if let Some(revision) = self.commit {
            if self.args.len() > 1 {
                bail!("--commit accepts at most one path");
            }
            return Ok(LaunchTarget::Commit {
                revision,
                path: self.args.into_iter().next().map(PathBuf::from),
            });
        }

        if self.staged {
            if self.args.len() != 1 {
                bail!("--staged requires exactly one path");
            }
            return Ok(LaunchTarget::Staged {
                path: PathBuf::from(self.args.into_iter().next().unwrap()),
            });
        }

        match self.args.as_slice() {
            [path] => Ok(LaunchTarget::WorkingTree {
                path: PathBuf::from(path),
            }),
            [left, right] => Ok(LaunchTarget::Compare {
                left: left.clone(),
                right: right.clone(),
                path: None,
            }),
            [left, right, path] => Ok(LaunchTarget::Compare {
                left: left.clone(),
                right: right.clone(),
                path: Some(PathBuf::from(path)),
            }),
            [] => bail!("diff requires a path, --commit REV, or two revisions"),
            _ => unreachable!("clap limits diff arguments"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_worktree_diff() {
        let cli = Cli::try_parse_from(["raccoon", "diff", "a file.rs"]).unwrap();
        assert_eq!(
            cli.launch_target().unwrap(),
            LaunchTarget::WorkingTree {
                path: "a file.rs".into()
            }
        );
    }

    #[test]
    fn rejects_staged_without_path() {
        let cli = Cli::try_parse_from(["raccoon", "diff", "--staged"]).unwrap();
        assert!(
            cli.launch_target()
                .unwrap_err()
                .to_string()
                .contains("exactly one")
        );
    }
}
