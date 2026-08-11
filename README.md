# Raccoon

Raccoon is a read-only, performance-focused Git TUI for reaching and inspecting diffs quickly. It targets Ghostty true color while remaining portable through Crossterm.

Repository and Material-style file icons use Nerd Font glyphs. Configure Ghostty or
your terminal with a Nerd Font for the intended appearance.

Changed files are grouped into expanded directory trees while selection and opening
remain file-oriented.

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

### History

- `Tab`, `←`, `→`: change panel
- `j`, `k`, arrows, Page Up/Down: move selection
- `Enter`: inspect the selected commit or file changed by that commit
- `u`: open uncommitted changes
- `b`: open the branch picker
- `c`: open the branch picker with the current branch selected as the comparison base
- `r`: refresh repository data
- `t`: preview themes
- `q`: quit

### Uncommitted changes

- `Tab`, `←`, `→`: switch between staged and unstaged files
- `j`, `k`, arrows, Page Up/Down: move selection
- `Enter`: inspect the selected changed file
- `h`: return to history
- `b`: open the branch picker
- `r`: refresh repository data
- `q`: quit

### Branch picker

- `j`, `k`, arrows, Page Up/Down: move selection
- `Enter`: inspect the selected branch
- `c`: select the comparison base and target
- `b` or `Esc`: close the picker

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

To add a theme:

1. Copy `night-owl.toml` and change its palette and unique `name`.
2. For a personal theme, place the file in the `themes/` directory beside Raccoon's generated `config.toml`.
3. For a theme bundled with Raccoon, add the TOML file to `assets/themes/`. The build discovers every TOML file there automatically, so no Rust registry change is needed.

The semantic mappings keep rendering independent of a specific palette. For example, a future Dracula theme only needs `assets/themes/dracula.toml`; UI and diff code require no changes.

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
