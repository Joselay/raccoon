use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use raccoon::{
    app, cli::Cli, config::init_file_logging, repository::Repository, terminal::TerminalSession,
    theme::LoadedTheme,
};

fn main() {
    if let Err(error) = try_main() {
        eprintln!("error: {error:#}");
        std::process::exit(2);
    }
}

fn try_main() -> Result<()> {
    let cli = Cli::parse();
    let has_repository_override = cli.repo.is_some();
    let repository_hint = cli.repo.clone();
    let target = cli.launch_target()?;
    let invocation_dir = env::current_dir().context("read current directory")?;
    let discovery_path = repository_hint.unwrap_or_else(|| PathBuf::from(&invocation_dir));
    let repo = Repository::discover(&discovery_path)?;
    let path_base = if has_repository_override {
        &repo.root
    } else {
        &invocation_dir
    };
    let target = repo.validate_target(target, path_base)?;
    let loaded_theme =
        LoadedTheme::discover().context("resolve Raccoon configuration directory")?;
    init_file_logging(&loaded_theme.paths).context("initialize diagnostics")?;
    for diagnostic in &loaded_theme.diagnostics {
        tracing::warn!(
            message = %diagnostic.message,
            path = ?diagnostic.path,
            "theme diagnostic"
        );
        eprintln!("warning: {}", diagnostic.message);
    }

    // Everything above this line is deliberately completed before terminal mode changes.
    let mut session = TerminalSession::enter()?;
    app::run(session.terminal_mut(), repo, target, loaded_theme)
}
