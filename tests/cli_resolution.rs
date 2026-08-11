use std::{fs, path::Path, process::Command};

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use raccoon::{
    cli::LaunchTarget,
    dashboard::{ChangeKind, load as load_dashboard, load_commit_files},
    git_diff,
    repository::Repository,
};
use tempfile::TempDir;

fn repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    fs::write(dir.path().join("tracked file.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "initial",
        ],
    );
    dir
}

fn git(directory: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn discovers_from_nested_directory_and_resolves_space_in_path() {
    let temp = repo();
    let nested = temp.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    let repository = Repository::discover(&nested).unwrap();
    let target = repository
        .validate_target(
            LaunchTarget::WorkingTree {
                path: Path::new("../../tracked file.txt").into(),
            },
            &nested,
        )
        .unwrap();
    assert_eq!(
        target,
        LaunchTarget::WorkingTree {
            path: "tracked file.txt".into()
        }
    );
}

#[test]
fn invalid_revision_fails_before_terminal_startup() {
    let temp = repo();
    AssertCommand::cargo_bin("raccoon")
        .unwrap()
        .args([
            "--repo",
            temp.path().to_str().unwrap(),
            "show",
            "not-a-revision",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid commit revision"));
}

#[test]
fn missing_path_fails_before_terminal_startup() {
    let temp = repo();
    AssertCommand::cargo_bin("raccoon")
        .unwrap()
        .args([
            "--repo",
            temp.path().to_str().unwrap(),
            "diff",
            "missing.txt",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("path does not exist"));
}

#[test]
fn non_repository_is_actionable() {
    let temp = tempfile::tempdir().unwrap();
    AssertCommand::cargo_bin("raccoon")
        .unwrap()
        .args(["--repo", temp.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no Git repository found"));
}

#[test]
fn dashboard_loads_history_branches_and_separated_changes() {
    let temp = repo();
    fs::write(temp.path().join("tracked file.txt"), "one\nunstaged\n").unwrap();
    fs::write(temp.path().join("staged.txt"), "staged\n").unwrap();
    fs::write(temp.path().join("untracked.txt"), "untracked\n").unwrap();
    git(temp.path(), &["add", "staged.txt"]);

    let repository = Repository::discover(temp.path()).unwrap();
    let dashboard = load_dashboard(&repository, None).unwrap();

    assert_eq!(dashboard.commits.len(), 1);
    assert_eq!(dashboard.commits[0].subject, "initial");
    let current_branch = dashboard
        .branches
        .iter()
        .find(|branch| branch.current)
        .unwrap();
    assert_eq!(dashboard.head.branch.as_ref(), Some(&current_branch.name));
    assert!(dashboard.head.short_id.is_some());
    let commit_files = load_commit_files(&repository, &dashboard.commits[0].id, None).unwrap();
    assert!(commit_files.iter().any(|change| {
        change.path == Path::new("tracked file.txt") && change.kind == ChangeKind::Added
    }));
    assert!(dashboard.staged.iter().any(|change| {
        change.path == Path::new("staged.txt") && change.kind == ChangeKind::Added
    }));
    assert!(dashboard.unstaged.iter().any(|change| {
        change.path == Path::new("tracked file.txt") && change.kind == ChangeKind::Modified
    }));
    assert!(dashboard.unstaged.iter().any(|change| {
        change.path == Path::new("untracked.txt") && change.kind == ChangeKind::Untracked
    }));
}

#[test]
fn untracked_file_opens_as_new_file_diff() {
    let temp = repo();
    fs::write(temp.path().join("new file.txt"), "new\n").unwrap();
    let repository = Repository::discover(temp.path()).unwrap();
    let document = git_diff::load(
        &repository,
        &LaunchTarget::WorkingTree {
            path: "new file.txt".into(),
        },
    )
    .unwrap();

    assert!(document.lines.iter().any(|line| {
        line.kind == raccoon::diff::LineKind::Addition && document.line_text(line) == "+new"
    }));
}

#[test]
fn staged_commit_and_revision_comparison_diffs_are_correct() {
    let temp = repo();
    fs::write(temp.path().join("tracked file.txt"), "two\n").unwrap();
    git(temp.path(), &["add", "tracked file.txt"]);
    let repository = Repository::discover(temp.path()).unwrap();

    let staged = git_diff::load(
        &repository,
        &LaunchTarget::Staged {
            path: "tracked file.txt".into(),
        },
    )
    .unwrap();
    assert!(staged.lines.iter().any(|line| {
        line.kind == raccoon::diff::LineKind::Addition && staged.line_text(line) == "+two"
    }));

    git(
        temp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "second",
        ],
    );
    let shown = git_diff::load(
        &repository,
        &LaunchTarget::Commit {
            revision: "HEAD".into(),
            path: Some("tracked file.txt".into()),
        },
    )
    .unwrap();
    assert!(
        shown
            .lines
            .iter()
            .any(|line| shown.line_text(line).contains("Commit:"))
    );

    let compared = git_diff::load(
        &repository,
        &LaunchTarget::Compare {
            left: "HEAD~1".into(),
            right: "HEAD".into(),
            path: Some("tracked file.txt".into()),
        },
    )
    .unwrap();
    assert!(compared.lines.iter().any(|line| {
        line.kind == raccoon::diff::LineKind::Deletion && compared.line_text(line) == "-one"
    }));
}

#[test]
fn missing_git_is_reported_before_terminal_startup_for_direct_entry() {
    let temp = repo();
    AssertCommand::cargo_bin("raccoon")
        .unwrap()
        .env("PATH", "")
        .args(["--repo", temp.path().to_str().unwrap(), "show", "HEAD"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ensure Git is installed"));
}

#[cfg(unix)]
#[test]
fn preserves_non_utf8_repository_paths() {
    use std::os::unix::ffi::OsStringExt;

    let temp = repo();
    let name = std::ffi::OsString::from_vec(b"non-utf8-\xFF.txt".to_vec());
    if fs::write(temp.path().join(&name), "bytes\n").is_err() {
        // Some Unix filesystems (notably default macOS APFS setups) reject
        // non-UTF-8 names; the product requirement applies where permitted.
        return;
    }
    let repository = Repository::discover(temp.path()).unwrap();
    let target = repository
        .validate_target(
            LaunchTarget::WorkingTree {
                path: Path::new(&name).into(),
            },
            temp.path(),
        )
        .unwrap();
    let document = git_diff::load(&repository, &target).unwrap();
    assert!(
        document
            .lines
            .iter()
            .any(|line| document.line_text(line) == "+bytes")
    );
}
