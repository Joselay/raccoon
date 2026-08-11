use std::ffi::OsString;

use anyhow::{Result, bail};

use crate::{cli::LaunchTarget, diff::DiffDocument, repository::Repository};

pub fn load(repo: &Repository, target: &LaunchTarget) -> Result<DiffDocument> {
    let mut args = vec![OsString::from("--no-pager")];
    let mut working_tree_path = None;
    match target {
        LaunchTarget::WorkingTree { path } => {
            args.extend(["diff", "--no-color", "--no-ext-diff", "--"].map(OsString::from));
            args.push(path.as_os_str().to_owned());
            working_tree_path = Some(path);
        }
        LaunchTarget::Staged { path } => {
            args.extend(
                ["diff", "--cached", "--no-color", "--no-ext-diff", "--"].map(OsString::from),
            );
            args.push(path.as_os_str().to_owned());
        }
        LaunchTarget::Commit { revision, path } | LaunchTarget::Show { revision, path } => {
            args.extend(
                ["show", "--format=fuller", "--no-color", "--no-ext-diff"].map(OsString::from),
            );
            args.push(revision.clone());
            if let Some(path) = path {
                args.push("--".into());
                args.push(path.as_os_str().to_owned());
            }
        }
        LaunchTarget::Compare { left, right, path } => {
            args.extend(["diff", "--no-color", "--no-ext-diff"].map(OsString::from));
            args.push(left.clone());
            args.push(right.clone());
            if let Some(path) = path {
                args.push("--".into());
                args.push(path.as_os_str().to_owned());
            }
        }
        _ => bail!("launch target does not contain a diff"),
    }
    let mut output = repo.git(args)?;
    // Keep the common tracked-file path to one Git process. An empty normal
    // diff is ambiguous with an untracked file, so only then perform the
    // additional index check and synthesize a new-file diff.
    if output.status.success()
        && output.stdout.is_empty()
        && let Some(path) = working_tree_path
        && repo.root.join(path).exists()
    {
        let tracked = repo
            .git([
                OsString::from("ls-files"),
                OsString::from("--error-unmatch"),
                OsString::from("--"),
                path.as_os_str().to_owned(),
            ])?
            .status
            .success();
        if !tracked {
            output = repo.git([
                OsString::from("--no-pager"),
                OsString::from("diff"),
                OsString::from("--no-index"),
                OsString::from("--no-color"),
                OsString::from("--no-ext-diff"),
                OsString::from("--"),
                OsString::from(if cfg!(windows) { "NUL" } else { "/dev/null" }),
                path.as_os_str().to_owned(),
            ])?;
        }
    }
    let allowed_no_index_difference =
        matches!(target, LaunchTarget::WorkingTree { .. }) && output.status.code() == Some(1);
    if !output.status.success() && !allowed_no_index_difference {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(DiffDocument::parse(output.stdout))
}
