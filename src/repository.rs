use std::{
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, anyhow, bail};

use crate::cli::LaunchTarget;

#[derive(Debug, Clone)]
pub struct Repository {
    pub root: PathBuf,
}

impl Repository {
    pub fn discover(start: &Path) -> Result<Self> {
        let repo = gix::discover(start)
            .with_context(|| format!("no Git repository found from {}", start.display()))?;
        let root = repo
            .workdir()
            .ok_or_else(|| anyhow!("bare repositories are not supported yet"))?
            .canonicalize()
            .context("resolve repository worktree")?;
        Ok(Self { root })
    }

    pub fn validate_target(
        &self,
        target: LaunchTarget,
        invocation_dir: &Path,
    ) -> Result<LaunchTarget> {
        let resolve = |path: PathBuf| self.resolve_path(&path, invocation_dir);
        Ok(match target {
            LaunchTarget::WorkingTree { path } => LaunchTarget::WorkingTree {
                path: resolve(path)?,
            },
            LaunchTarget::Staged { path } => LaunchTarget::Staged {
                path: resolve(path)?,
            },
            LaunchTarget::Commit { revision, path } => {
                self.validate_revision(&revision)?;
                LaunchTarget::Commit {
                    revision,
                    path: path.map(resolve).transpose()?,
                }
            }
            LaunchTarget::Compare { left, right, path } => {
                self.validate_revision(&left)?;
                self.validate_revision(&right)?;
                LaunchTarget::Compare {
                    left,
                    right,
                    path: path.map(resolve).transpose()?,
                }
            }
            LaunchTarget::Show { revision, path } => {
                self.validate_revision(&revision)?;
                LaunchTarget::Show {
                    revision,
                    path: path.map(resolve).transpose()?,
                }
            }
            LaunchTarget::History { path } => LaunchTarget::History {
                path: path.map(resolve).transpose()?,
            },
            other => other,
        })
    }

    pub fn git_dir(&self) -> Result<PathBuf> {
        let output = self.git(["rev-parse", "--absolute-git-dir"])?;
        if !output.status.success() {
            bail!(
                "resolve Git directory: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let path = PathBuf::from(bytes_to_os_string(
            output
                .stdout
                .strip_suffix(b"\n")
                .unwrap_or(&output.stdout)
                .strip_suffix(b"\r")
                .unwrap_or(output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout)),
        ));
        path.canonicalize()
            .with_context(|| format!("resolve Git directory {}", path.display()))
    }

    fn resolve_path(&self, path: &Path, invocation_dir: &Path) -> Result<PathBuf> {
        let absolute = if path.is_absolute() {
            normalize(path)
        } else {
            let base = invocation_dir
                .canonicalize()
                .with_context(|| format!("resolve path base {}", invocation_dir.display()))?;
            normalize(&base.join(path))
        };
        let relative = absolute.strip_prefix(&self.root).with_context(|| {
            format!(
                "path {} is outside repository {}",
                path.display(),
                self.root.display()
            )
        })?;
        if relative.as_os_str().is_empty() {
            bail!("a file path is required; the repository root is not a file");
        }

        let exists = self.root.join(relative).exists();
        let tracked = self
            .git([
                OsStr::new("ls-files"),
                OsStr::new("--error-unmatch"),
                OsStr::new("--"),
                relative.as_os_str(),
            ])
            .map(|output| output.status.success())
            .unwrap_or(false);
        if !exists && !tracked {
            bail!("path does not exist in the repository: {}", path.display());
        }
        Ok(relative.to_path_buf())
    }

    fn validate_revision(&self, revision: &OsStr) -> Result<()> {
        let mut commit = OsString::from(revision);
        commit.push("^{commit}");
        let output = self.git([
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("--end-of-options"),
            commit.as_os_str(),
        ])?;
        if !output.status.success() {
            bail!(
                "invalid commit revision '{}': {}",
                revision.to_string_lossy(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn git<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .context("failed to run git; ensure Git is installed and available in PATH")
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

#[cfg(unix)]
fn bytes_to_os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn bytes_to_os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}
