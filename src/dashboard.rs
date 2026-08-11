use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, bail};

use crate::repository::Repository;

const HISTORY_LIMIT: &str = "500";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEntry {
    pub id: OsString,
    pub short_id: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchEntry {
    pub name: OsString,
    pub short_id: String,
    pub subject: String,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadInfo {
    pub branch: Option<OsString>,
    pub short_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Untracked,
    TypeChanged,
    Unmerged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEntry {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Default)]
pub struct DashboardData {
    pub commits: Vec<CommitEntry>,
    pub branches: Vec<BranchEntry>,
    pub staged: Vec<ChangeEntry>,
    pub unstaged: Vec<ChangeEntry>,
    pub head: HeadInfo,
}

pub fn load(repo: &Repository, history_path: Option<&PathBuf>) -> Result<DashboardData> {
    let commits = load_history(repo, history_path)?;
    let head = load_head(repo)?;
    let branches = load_branches(repo, head.branch.as_deref())?;
    let (staged, unstaged) = load_status(repo)?;
    Ok(DashboardData {
        commits,
        branches,
        staged,
        unstaged,
        head,
    })
}

pub fn load_history(repo: &Repository, path: Option<&PathBuf>) -> Result<Vec<CommitEntry>> {
    let mut args: Vec<OsString> = [
        "--no-pager",
        "log",
        "--date=short",
        "--pretty=format:%H%x00%h%x00%an%x00%ad%x00%s%x00",
        "-n",
        HISTORY_LIMIT,
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if let Some(path) = path {
        args.push("--".into());
        args.push(path.as_os_str().to_owned());
    }
    let output = repo.git(args)?;
    // An unborn repository has no history and is a valid dashboard state.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("does not have any commits yet")
            || stderr.contains("your current branch")
        {
            return Ok(Vec::new());
        }
        bail!("load commit history: {}", stderr.trim());
    }
    let fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    Ok(fields
        .chunks_exact(5)
        .map(|field| CommitEntry {
            id: bytes_to_os_string(field[0].strip_prefix(b"\n").unwrap_or(field[0])),
            short_id: String::from_utf8_lossy(field[1]).into_owned(),
            author: String::from_utf8_lossy(field[2]).into_owned(),
            date: String::from_utf8_lossy(field[3]).into_owned(),
            subject: String::from_utf8_lossy(field[4])
                .trim_end_matches('\n')
                .to_owned(),
        })
        .collect())
}

pub fn load_head(repo: &Repository) -> Result<HeadInfo> {
    let branch_output = repo.git(["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = branch_output
        .status
        .success()
        .then(|| bytes_to_os_string(&trim_ascii(&branch_output.stdout)));
    let revision_output = repo.git(["rev-parse", "--verify", "--short", "HEAD"])?;
    let short_id = revision_output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&trim_ascii(&revision_output.stdout)).into_owned());
    Ok(HeadInfo { branch, short_id })
}

pub fn load_branches(
    repo: &Repository,
    current: Option<&std::ffi::OsStr>,
) -> Result<Vec<BranchEntry>> {
    let output = repo.git([
        "for-each-ref",
        "--format=%(refname:short)%00%(objectname:short)%00%(subject)%00",
        "refs/heads",
    ])?;
    if !output.status.success() {
        bail!(
            "load branches: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    Ok(fields
        .chunks_exact(3)
        .map(|field| {
            let name_bytes = field[0].strip_prefix(b"\n").unwrap_or(field[0]);
            let name = bytes_to_os_string(name_bytes);
            BranchEntry {
                current: current == Some(name.as_os_str()),
                name,
                short_id: String::from_utf8_lossy(field[1]).into_owned(),
                subject: String::from_utf8_lossy(field[2]).into_owned(),
            }
        })
        .collect())
}

pub fn load_commit_files(
    repo: &Repository,
    revision: &std::ffi::OsStr,
    path: Option<&PathBuf>,
) -> Result<Vec<ChangeEntry>> {
    let mut args: Vec<OsString> = [
        "diff-tree",
        "--root",
        "--first-parent",
        "--no-commit-id",
        "--name-status",
        "-r",
        "-z",
        "-M",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(revision.to_owned());
    if let Some(path) = path {
        args.push("--".into());
        args.push(path.as_os_str().to_owned());
    }
    let output = repo.git(args)?;
    if !output.status.success() {
        bail!(
            "load files changed by commit: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let records = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let status = records[index];
        index += 1;
        if status.is_empty() || index >= records.len() {
            continue;
        }
        let kind = status_kind(status[0]);
        let mut path = PathBuf::from(bytes_to_os_string(records[index]));
        index += 1;
        if matches!(status[0], b'R' | b'C') && index < records.len() {
            path = PathBuf::from(bytes_to_os_string(records[index]));
            index += 1;
        }
        files.push(ChangeEntry { path, kind });
    }
    Ok(files)
}

pub fn load_status(repo: &Repository) -> Result<(Vec<ChangeEntry>, Vec<ChangeEntry>)> {
    let output = repo.git([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ])?;
    if !output.status.success() {
        bail!(
            "load working tree status: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let records = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() || record.len() < 4 {
            continue;
        }
        let x = record[0];
        let y = record[1];
        let path = PathBuf::from(bytes_to_os_string(&record[3..]));
        if matches!(x, b'R' | b'C') {
            // Porcelain v1 -z includes a second path for copies and renames.
            index += 1;
        }
        if x == b'?' && y == b'?' {
            unstaged.push(ChangeEntry {
                path,
                kind: ChangeKind::Untracked,
            });
            continue;
        }
        if x != b' ' {
            staged.push(ChangeEntry {
                path: path.clone(),
                kind: status_kind(x),
            });
        }
        if y != b' ' {
            unstaged.push(ChangeEntry {
                path,
                kind: status_kind(y),
            });
        }
    }
    Ok((staged, unstaged))
}

fn status_kind(code: u8) -> ChangeKind {
    match code {
        b'A' => ChangeKind::Added,
        b'M' => ChangeKind::Modified,
        b'D' => ChangeKind::Deleted,
        b'R' => ChangeKind::Renamed,
        b'C' => ChangeKind::Copied,
        b'T' => ChangeKind::TypeChanged,
        b'U' => ChangeKind::Unmerged,
        b'?' => ChangeKind::Untracked,
        _ => ChangeKind::Unknown,
    }
}

fn trim_ascii(bytes: &[u8]) -> Vec<u8> {
    bytes
        .strip_suffix(b"\n")
        .unwrap_or(bytes)
        .strip_suffix(b"\r")
        .unwrap_or(bytes.strip_suffix(b"\n").unwrap_or(bytes))
        .to_vec()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_status_codes() {
        assert_eq!(status_kind(b'A'), ChangeKind::Added);
        assert_eq!(status_kind(b'R'), ChangeKind::Renamed);
        assert_eq!(status_kind(b'?'), ChangeKind::Untracked);
    }
}
