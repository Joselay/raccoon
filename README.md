# Raccoon

Raccoon is a read-only, performance-focused Git TUI for reaching and inspecting diffs quickly. It targets Ghostty true color while remaining portable through Crossterm.

## Install

```sh
cargo install --path .
```

Raccoon requires an installed `git` executable. The default syntax backend uses Oniguruma; a pure-Rust build is available with:

```sh
cargo install --path . --no-default-features --features fancy
```

## Usage

```text
raccoon
raccoon --repo /path/to/repository
raccoon diff path/to/file.rs
raccoon diff --staged path/to/file.rs
raccoon diff --commit HEAD~1 [path]
raccoon diff main feature [path]
raccoon show HEAD [path]
raccoon history [path]
raccoon branches
raccoon changes
```

Paths are interpreted relative to the invocation directory, or relative to the supplied `--repo` worktree. Repository, revision, path, and flag errors are validated before alternate-screen mode.

## Keys

### Dashboard

- `Tab`, `←`, `→`: change panel
- `j`, `k`, arrows, Page Up/Down: move selection
- `Enter`: inspect selected commit, branch, or changed file
- `c`: select two branches for comparison
- `r`: refresh repository data
- `t`: preview themes
- `q`: quit

### Diff

- `j`, `k`, arrows, Page Up/Down: scroll
- `n`, `N`: next/previous hunk
- `]`, `[`: next/previous file
- `/`: search; `s`/`S`: next/previous result
- `b` or `Esc`: return to the dashboard
- `t`: preview themes
- `q`: quit

Git operations and syntax highlighting run in bounded background workers. Semantic diff coloring appears before syntax highlighting completes.

## Themes

Night Owl is Raccoon's bundled dark theme and the default. Press `t` to preview custom themes, `Enter` to persist one, or `Esc` to cancel.

Custom themes are loaded from the platform configuration directory's `themes/` folder. Theme files use named TOML palette references and must provide every UI, diff, and syntax semantic color, declare `appearance = "dark"`, and pass contrast validation. Invalid themes are reported and safely fall back to Night Owl.

See [`assets/themes/night-owl.toml`](assets/themes/night-owl.toml) for the schema.

## Safety and scope

Raccoon is read-only. Branch selection never checks out or modifies a branch. Stashes, tags, reflog, worktrees, light themes, and general Git feature parity are intentionally out of scope.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo bench --bench diff
cargo build --release
```
