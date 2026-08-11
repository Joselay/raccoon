use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing_subscriber::util::SubscriberInitExt;

use crate::theme::DEFAULT_THEME_NAME;

/// Platform-specific locations used by Raccoon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub root: PathBuf,
    pub config_file: PathBuf,
    pub themes_dir: PathBuf,
}

impl ConfigPaths {
    /// Resolve the application configuration directory with `directories`.
    pub fn discover() -> Result<Self, ConfigError> {
        let dirs = ProjectDirs::from("dev", "Raccoon", "Raccoon")
            .ok_or(ConfigError::ConfigDirectoryUnavailable)?;
        Ok(Self::from_root(dirs.config_dir()))
    }

    /// Construct paths from a known root. Useful for portable installs and tests.
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            config_file: root.join("config.toml"),
            themes_dir: root.join("themes"),
            root,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    pub name: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: DEFAULT_THEME_NAME.to_owned(),
        }
    }
}

impl AppConfig {
    /// Load `config.toml`; a missing file means the default configuration.
    pub fn load(paths: &ConfigPaths) -> Result<Self, ConfigError> {
        match fs::read_to_string(&paths.config_file) {
            Ok(source) => toml::from_str(&source).map_err(|source| ConfigError::Parse {
                path: paths.config_file.clone(),
                source,
            }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                operation: "read",
                path: paths.config_file.clone(),
                source,
            }),
        }
    }

    /// Persist the selected theme. Call this only after a preview is confirmed.
    pub fn save(&self, paths: &ConfigPaths) -> Result<(), ConfigError> {
        fs::create_dir_all(&paths.root).map_err(|source| ConfigError::Io {
            operation: "create",
            path: paths.root.clone(),
            source,
        })?;
        let source = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        fs::write(&paths.config_file, source).map_err(|source| ConfigError::Io {
            operation: "write",
            path: paths.config_file.clone(),
            source,
        })
    }
}

/// Initialize diagnostics in the configuration directory without ever writing
/// log records to the active terminal.
pub fn init_file_logging(paths: &ConfigPaths) -> Result<(), ConfigError> {
    fs::create_dir_all(&paths.root).map_err(|source| ConfigError::Io {
        operation: "create",
        path: paths.root.clone(),
        source,
    })?;
    let path = paths.root.join("raccoon.log");
    let file = fs::File::options()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| ConfigError::Io {
            operation: "open",
            path: path.clone(),
            source,
        })?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(true)
        .with_writer(Mutex::new(file))
        .finish()
        .try_init()
        .map_err(|source| ConfigError::Logging(source.to_string()))
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("the platform configuration directory is unavailable")]
    ConfigDirectoryUnavailable,
    #[error("could not {operation} configuration path {path}: {source}", path = .path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid configuration in {path}: {source}", path = .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("could not serialize configuration: {0}")]
    Serialize(#[source] toml::ser::Error),
    #[error("could not initialize file diagnostics: {0}")]
    Logging(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_uses_default_theme() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::from_root(temp.path());
        assert_eq!(
            AppConfig::load(&paths).unwrap().theme.name,
            DEFAULT_THEME_NAME
        );
    }

    #[test]
    fn selected_theme_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::from_root(temp.path());
        let config = AppConfig {
            theme: ThemeConfig {
                name: "Nord".into(),
            },
        };
        config.save(&paths).unwrap();
        assert_eq!(AppConfig::load(&paths).unwrap(), config);
    }
}
