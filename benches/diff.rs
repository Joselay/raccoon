use std::{fs, hint::black_box, num::NonZeroUsize, path::Path, process::Command};

use criterion::{Criterion, criterion_group, criterion_main};
use lru::LruCache;
use raccoon::{
    cli::LaunchTarget, dashboard, diff::DiffDocument, git_diff, highlight, repository::Repository,
    theme::Theme,
};

fn parse_diff(c: &mut Criterion) {
    let mut input = String::from(
        "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ -1,10000 +1,10000 @@\n",
    );
    for line in 0..10_000 {
        input.push_str(&format!("+let value_{line} = {line};\n"));
    }
    let bytes = input.into_bytes();
    c.bench_function("parse_10k_line_diff", |b| {
        b.iter(|| DiffDocument::parse(black_box(bytes.clone())))
    });
}

fn direct_diff_startup(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    fs::write(temp.path().join("file.txt"), "before\n").unwrap();
    git(temp.path(), &["add", "."]);
    git(
        temp.path(),
        &[
            "-c",
            "user.name=Bench",
            "-c",
            "user.email=bench@example.com",
            "commit",
            "-qm",
            "initial",
        ],
    );
    fs::write(temp.path().join("file.txt"), "after\n").unwrap();
    let repo = Repository::discover(temp.path()).unwrap();
    let target = LaunchTarget::WorkingTree {
        path: "file.txt".into(),
    };

    c.bench_function("direct_diff_git_and_parse", |b| {
        b.iter(|| black_box(git_diff::load(&repo, &target).unwrap()))
    });
}

fn visible_syntax_highlighting(c: &mut Criterion) {
    let mut input = String::from(
        "diff --git a/main.rs b/main.rs\n--- a/main.rs\n+++ b/main.rs\n@@ -0,0 +1,200 @@\n",
    );
    for line in 0..200 {
        input.push_str(&format!(
            "+fn function_{line}() {{ println!(\"{line}\"); }}\n"
        ));
    }
    let document = DiffDocument::parse(input.into_bytes());
    let highlighter = highlight::Highlighter::new();
    c.bench_function("highlight_200_visible_rust_lines", |b| {
        b.iter(|| black_box(highlighter.highlight(black_box(&document)).unwrap()))
    });
}

fn theme_loading(c: &mut Criterion) {
    let source = include_str!("../assets/themes/night-owl.toml");
    c.bench_function("parse_and_validate_theme", |b| {
        b.iter(|| black_box(Theme::from_toml(black_box(source)).unwrap()))
    });
}

fn cache_access(c: &mut Criterion) {
    let mut cache = LruCache::new(NonZeroUsize::new(128).unwrap());
    for key in 0..128 {
        cache.put(key, key * 2);
    }
    c.bench_function("bounded_lru_cache_hit", |b| {
        b.iter(|| black_box(cache.get(black_box(&64)).copied()))
    });
}

fn commit_and_status_loading(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    fs::write(temp.path().join("file.txt"), "before\n").unwrap();
    git(temp.path(), &["add", "."]);
    git(
        temp.path(),
        &[
            "-c",
            "user.name=Bench",
            "-c",
            "user.email=bench@example.com",
            "commit",
            "-qm",
            "initial",
        ],
    );
    fs::write(temp.path().join("file.txt"), "after\n").unwrap();
    let repo = Repository::discover(temp.path()).unwrap();
    c.bench_function("load_commit_branch_and_status_dashboard", |b| {
        b.iter(|| black_box(dashboard::load(black_box(&repo), None).unwrap()))
    });
}

fn large_repository_dashboard(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "gc.auto", "0"]);
    for index in 0..200 {
        fs::write(temp.path().join("history.txt"), format!("{index}\n")).unwrap();
        git(temp.path(), &["add", "history.txt"]);
        git(
            temp.path(),
            &[
                "-c",
                "user.name=Bench",
                "-c",
                "user.email=bench@example.com",
                "commit",
                "-qm",
                &format!("commit {index}"),
            ],
        );
    }
    for index in 0..200 {
        fs::write(
            temp.path().join(format!("changed-{index:03}.txt")),
            format!("change {index}\n"),
        )
        .unwrap();
    }
    let repo = Repository::discover(temp.path()).unwrap();
    let mut group = c.benchmark_group("large_repository");
    group.sample_size(20);
    group.bench_function("load_200_commits_and_200_changes", |b| {
        b.iter(|| black_box(dashboard::load(black_box(&repo), None).unwrap()))
    });
    group.finish();
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

criterion_group!(
    benches,
    parse_diff,
    direct_diff_startup,
    visible_syntax_highlighting,
    theme_loading,
    cache_access,
    commit_and_status_loading,
    large_repository_dashboard
);
criterion_main!(benches);
