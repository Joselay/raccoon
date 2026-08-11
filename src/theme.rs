use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::Deserialize;
use thiserror::Error;

use crate::config::{AppConfig, ConfigPaths};

pub const DEFAULT_THEME_NAME: &str = "Night Owl";
pub const MIN_TEXT_CONTRAST: f64 = 4.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub fn contrast_ratio(self, other: Self) -> f64 {
        let (lighter, darker) = match (self.relative_luminance(), other.relative_luminance()) {
            (a, b) if a >= b => (a, b),
            (a, b) => (b, a),
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    fn relative_luminance(self) -> f64 {
        fn channel(value: u8) -> f64 {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.red) + 0.7152 * channel(self.green) + 0.0722 * channel(self.blue)
    }
}

impl FromStr for Rgb {
    type Err = ColorParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 7
            || !value.starts_with('#')
            || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ColorParseError(value.to_owned()));
        }
        Ok(Self {
            red: u8::from_str_radix(&value[1..3], 16).expect("validated hex"),
            green: u8::from_str_radix(&value[3..5], 16).expect("validated hex"),
            blue: u8::from_str_radix(&value[5..7], 16).expect("validated hex"),
        })
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "#{:02X}{:02X}{:02X}",
            self.red, self.green, self.blue
        )
    }
}

impl From<Rgb> for ratatui::style::Color {
    fn from(value: Rgb) -> Self {
        Self::Rgb(value.red, value.green, value.blue)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub appearance: Appearance,
    pub ui: UiColors,
    pub diff: DiffColors,
    pub syntax: SyntaxColors,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiColors {
    pub background: Rgb,
    pub foreground: Rgb,
    pub panel: Rgb,
    pub border: Rgb,
    pub focused_border: Rgb,
    pub selection: Rgb,
    pub selection_foreground: Rgb,
    pub muted: Rgb,
    pub accent: Rgb,
    pub warning: Rgb,
    pub error: Rgb,
    pub info: Rgb,
    pub search_match: Rgb,
    pub search_match_foreground: Rgb,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffColors {
    pub header: Rgb,
    pub hunk_header: Rgb,
    pub gutter: Rgb,
    pub line_number: Rgb,
    pub addition: Rgb,
    pub deletion: Rgb,
    pub context: Rgb,
    pub metadata: Rgb,
    pub addition_background: Rgb,
    pub deletion_background: Rgb,
    pub selected_background: Rgb,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxColors {
    pub comment: Rgb,
    pub keyword: Rgb,
    pub string: Rgb,
    pub number: Rgb,
    pub function: Rgb,
    pub r#type: Rgb,
    pub variable: Rgb,
    pub constant: Rgb,
    pub operator: Rgb,
    pub punctuation: Rgb,
    pub property: Rgb,
    pub tag: Rgb,
    pub attribute: Rgb,
    pub invalid: Rgb,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    appearance: String,
    palette: BTreeMap<String, String>,
    ui: UiRefs,
    diff: DiffRefs,
    syntax: SyntaxRefs,
}

macro_rules! refs_struct {
    ($name:ident { $($field:ident),+ $(,)? }) => {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $name { $( $field: String, )+ }
    };
}

refs_struct!(UiRefs {
    background,
    foreground,
    panel,
    border,
    focused_border,
    selection,
    selection_foreground,
    muted,
    accent,
    warning,
    error,
    info,
    search_match,
    search_match_foreground
});
refs_struct!(DiffRefs {
    header,
    hunk_header,
    gutter,
    line_number,
    addition,
    deletion,
    context,
    metadata,
    addition_background,
    deletion_background,
    selected_background
});
refs_struct!(SyntaxRefs {
    comment,
    keyword,
    string,
    number,
    function,
    r#type,
    variable,
    constant,
    operator,
    punctuation,
    property,
    tag,
    attribute,
    invalid
});

impl Theme {
    pub fn from_toml(source: &str) -> Result<Self, ThemeError> {
        let file: ThemeFile = toml::from_str(source)?;
        if file.appearance != "dark" {
            return Err(ThemeError::UnsupportedAppearance(file.appearance));
        }
        if file.name.trim().is_empty() {
            return Err(ThemeError::EmptyName);
        }

        let mut palette = BTreeMap::new();
        for (name, value) in file.palette {
            let color = value.parse().map_err(|_| ThemeError::InvalidPaletteColor {
                name: name.clone(),
                value,
            })?;
            palette.insert(name, color);
        }
        let resolve = |section: &'static str, semantic: &'static str, reference: &str| {
            palette
                .get(reference)
                .copied()
                .ok_or_else(|| ThemeError::UnknownPaletteReference {
                    section,
                    semantic,
                    reference: reference.to_owned(),
                })
        };
        macro_rules! color {
            ($section:literal, $refs:expr, $field:ident) => {
                resolve($section, stringify!($field), &($refs).$field)?
            };
        }
        let ui = UiColors {
            background: color!("ui", file.ui, background),
            foreground: color!("ui", file.ui, foreground),
            panel: color!("ui", file.ui, panel),
            border: color!("ui", file.ui, border),
            focused_border: color!("ui", file.ui, focused_border),
            selection: color!("ui", file.ui, selection),
            selection_foreground: color!("ui", file.ui, selection_foreground),
            muted: color!("ui", file.ui, muted),
            accent: color!("ui", file.ui, accent),
            warning: color!("ui", file.ui, warning),
            error: color!("ui", file.ui, error),
            info: color!("ui", file.ui, info),
            search_match: color!("ui", file.ui, search_match),
            search_match_foreground: color!("ui", file.ui, search_match_foreground),
        };
        let diff = DiffColors {
            header: color!("diff", file.diff, header),
            hunk_header: color!("diff", file.diff, hunk_header),
            gutter: color!("diff", file.diff, gutter),
            line_number: color!("diff", file.diff, line_number),
            addition: color!("diff", file.diff, addition),
            deletion: color!("diff", file.diff, deletion),
            context: color!("diff", file.diff, context),
            metadata: color!("diff", file.diff, metadata),
            addition_background: color!("diff", file.diff, addition_background),
            deletion_background: color!("diff", file.diff, deletion_background),
            selected_background: color!("diff", file.diff, selected_background),
        };
        let syntax = SyntaxColors {
            comment: color!("syntax", file.syntax, comment),
            keyword: color!("syntax", file.syntax, keyword),
            string: color!("syntax", file.syntax, string),
            number: color!("syntax", file.syntax, number),
            function: color!("syntax", file.syntax, function),
            r#type: color!("syntax", file.syntax, r#type),
            variable: color!("syntax", file.syntax, variable),
            constant: color!("syntax", file.syntax, constant),
            operator: color!("syntax", file.syntax, operator),
            punctuation: color!("syntax", file.syntax, punctuation),
            property: color!("syntax", file.syntax, property),
            tag: color!("syntax", file.syntax, tag),
            attribute: color!("syntax", file.syntax, attribute),
            invalid: color!("syntax", file.syntax, invalid),
        };
        let theme = Self {
            name: file.name,
            appearance: Appearance::Dark,
            ui,
            diff,
            syntax,
        };
        theme.validate_contrast()?;
        Ok(theme)
    }

    pub fn validate_contrast(&self) -> Result<(), ThemeError> {
        self.require_contrast(
            "ui.foreground",
            self.ui.foreground,
            "ui.background",
            self.ui.background,
        )?;
        self.require_contrast(
            "ui.selection_foreground",
            self.ui.selection_foreground,
            "ui.selection",
            self.ui.selection,
        )?;
        self.require_contrast(
            "ui.search_match_foreground",
            self.ui.search_match_foreground,
            "ui.search_match",
            self.ui.search_match,
        )?;
        self.require_contrast(
            "ui.selection_foreground",
            self.ui.selection_foreground,
            "diff.selected_background",
            self.diff.selected_background,
        )?;
        self.require_contrast(
            "diff.context",
            self.diff.context,
            "ui.background",
            self.ui.background,
        )?;
        self.require_contrast(
            "diff.addition",
            self.diff.addition,
            "diff.addition_background",
            self.diff.addition_background,
        )?;
        self.require_contrast(
            "diff.deletion",
            self.diff.deletion,
            "diff.deletion_background",
            self.diff.deletion_background,
        )
    }

    fn require_contrast(
        &self,
        foreground_name: &'static str,
        foreground: Rgb,
        background_name: &'static str,
        background: Rgb,
    ) -> Result<(), ThemeError> {
        let ratio = foreground.contrast_ratio(background);
        if ratio < MIN_TEXT_CONTRAST {
            Err(ThemeError::InsufficientContrast {
                foreground: foreground_name,
                background: background_name,
                ratio,
                minimum: MIN_TEXT_CONTRAST,
            })
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Error)]
pub enum ThemeError {
    #[error("invalid theme TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("theme appearance must be `dark`; `{0}` themes are not supported")]
    UnsupportedAppearance(String),
    #[error("theme name must not be empty")]
    EmptyName,
    #[error("palette color `{name}` must use #RRGGBB, got `{value}`")]
    InvalidPaletteColor { name: String, value: String },
    #[error(
        "{section}.{semantic} references missing palette color `{reference}`; add it to [palette] or correct the reference"
    )]
    UnknownPaletteReference {
        section: &'static str,
        semantic: &'static str,
        reference: String,
    },
    #[error(
        "insufficient contrast between {foreground} and {background}: {ratio:.2}:1 (minimum {minimum:.1}:1)"
    )]
    InsufficientContrast {
        foreground: &'static str,
        background: &'static str,
        ratio: f64,
        minimum: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("color must use exactly #RRGGBB, got `{0}`")]
pub struct ColorParseError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeDiagnostic {
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ThemeCatalog {
    themes: BTreeMap<String, Theme>,
    pub diagnostics: Vec<ThemeDiagnostic>,
}

impl ThemeCatalog {
    pub fn bundled() -> Self {
        let mut themes = BTreeMap::new();
        for theme in bundled_themes() {
            let name = theme.name.clone();
            assert!(
                themes.insert(name.clone(), theme).is_none(),
                "bundled theme name `{name}` is duplicated"
            );
        }
        assert!(
            themes.contains_key(DEFAULT_THEME_NAME),
            "bundled default theme `{DEFAULT_THEME_NAME}` is missing"
        );
        Self {
            themes,
            diagnostics: Vec::new(),
        }
    }

    /// Load valid `.toml` files from a custom themes directory. Invalid files are
    /// reported and skipped, so the bundled default always remains available.
    pub fn load_custom(themes_dir: impl AsRef<Path>) -> Self {
        let mut catalog = Self::bundled();
        let themes_dir = themes_dir.as_ref();
        let entries = match fs::read_dir(themes_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return catalog,
            Err(error) => {
                catalog.diagnostics.push(ThemeDiagnostic {
                    path: Some(themes_dir.to_owned()),
                    message: format!("could not read themes directory: {error}"),
                });
                return catalog;
            }
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            catalog.load_file(path);
        }
        catalog
    }

    fn load_file(&mut self, path: PathBuf) {
        let result = fs::read_to_string(&path)
            .map_err(|error| format!("could not read theme: {error}"))
            .and_then(|source| Theme::from_toml(&source).map_err(|error| error.to_string()));
        match result {
            Ok(theme) if self.themes.contains_key(&theme.name) => {
                self.diagnostics.push(ThemeDiagnostic {
                    path: Some(path),
                    message: format!(
                        "theme name `{}` duplicates a bundled or previously loaded theme",
                        theme.name
                    ),
                })
            }
            Ok(theme) => {
                self.themes.insert(theme.name.clone(), theme);
            }
            Err(message) => self.diagnostics.push(ThemeDiagnostic {
                path: Some(path),
                message,
            }),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Theme> {
        self.themes.get(name)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Theme> {
        self.themes.values()
    }

    pub fn select_or_default(&self, name: &str) -> ThemeSelection {
        match self.get(name) {
            Some(theme) => ThemeSelection {
                theme: theme.clone(),
                used_fallback: false,
                warning: None,
            },
            None => ThemeSelection {
                theme: self
                    .themes
                    .get(DEFAULT_THEME_NAME)
                    .expect("bundled default theme is present")
                    .clone(),
                used_fallback: true,
                warning: Some(format!(
                    "theme `{name}` was not found; using {DEFAULT_THEME_NAME}"
                )),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThemeSelection {
    pub theme: Theme,
    pub used_fallback: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedTheme {
    pub paths: ConfigPaths,
    pub catalog: ThemeCatalog,
    pub selected: Theme,
    pub used_fallback: bool,
    pub diagnostics: Vec<ThemeDiagnostic>,
}

impl LoadedTheme {
    pub fn discover() -> Result<Self, crate::config::ConfigError> {
        ConfigPaths::discover().map(Self::load)
    }

    /// Load custom themes and the configured selection, safely falling back to
    /// Night Owl when configuration or selection is invalid.
    pub fn load(paths: ConfigPaths) -> Self {
        let catalog = ThemeCatalog::load_custom(&paths.themes_dir);
        let mut diagnostics = catalog.diagnostics.clone();
        let (configured, config_fallback) = match AppConfig::load(&paths) {
            Ok(config) => (config.theme.name, false),
            Err(error) => {
                diagnostics.push(ThemeDiagnostic {
                    path: Some(paths.config_file.clone()),
                    message: error.to_string(),
                });
                (DEFAULT_THEME_NAME.to_owned(), true)
            }
        };
        let selection = catalog.select_or_default(&configured);
        if let Some(message) = selection.warning.clone() {
            diagnostics.push(ThemeDiagnostic {
                path: Some(paths.config_file.clone()),
                message,
            });
        }
        Self {
            paths,
            catalog,
            selected: selection.theme,
            used_fallback: selection.used_fallback || config_fallback,
            diagnostics,
        }
    }
}

pub fn bundled_themes() -> Vec<Theme> {
    const SOURCES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/bundled_themes.rs"));
    SOURCES
        .iter()
        .map(|(file, source)| {
            Theme::from_toml(source)
                .unwrap_or_else(|error| panic!("bundled theme `{file}` must be valid: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(appearance: &str) -> String {
        format!(
            r##"name = "Test"
appearance = "{appearance}"
[palette]
bg = "#000000"
fg = "#FFFFFF"
red = "#FF8080"
green = "#80FF80"
blue = "#80C0FF"
mid = "#333333"
yellow = "#FFFF80"
[ui]
background="bg"
foreground="fg"
panel="bg"
border="mid"
focused_border="blue"
selection="mid"
selection_foreground="fg"
muted="fg"
accent="blue"
warning="yellow"
error="red"
info="blue"
search_match="yellow"
search_match_foreground="bg"
[diff]
header="blue"
hunk_header="blue"
gutter="mid"
line_number="fg"
addition="green"
deletion="red"
context="fg"
metadata="fg"
addition_background="bg"
deletion_background="bg"
selected_background="mid"
[syntax]
comment="fg"
keyword="blue"
string="green"
number="yellow"
function="blue"
type="blue"
variable="fg"
constant="yellow"
operator="fg"
punctuation="fg"
property="blue"
tag="blue"
attribute="yellow"
invalid="red"
"##
        )
    }

    #[test]
    fn parses_exact_hex_colors() {
        assert_eq!("#1a2B3c".parse::<Rgb>().unwrap(), Rgb::new(26, 43, 60));
        for invalid in ["1A2B3C", "#fff", "#1234567", "#GG0000"] {
            assert!(invalid.parse::<Rgb>().is_err());
        }
    }

    #[test]
    fn rejects_light_and_missing_references() {
        assert!(matches!(
            Theme::from_toml(&minimal("light")),
            Err(ThemeError::UnsupportedAppearance(_))
        ));
        let missing = minimal("dark").replace("foreground=\"fg\"", "foreground=\"missing\"");
        assert!(matches!(
            Theme::from_toml(&missing),
            Err(ThemeError::UnknownPaletteReference { .. })
        ));
        let missing_semantic = minimal("dark").replace("invalid=\"red\"\n", "");
        let error = Theme::from_toml(&missing_semantic).unwrap_err().to_string();
        assert!(error.contains("missing field") && error.contains("invalid"));
    }

    #[test]
    fn rejects_low_contrast() {
        let source = minimal("dark").replace("fg = \"#FFFFFF\"", "fg = \"#222222\"");
        assert!(matches!(
            Theme::from_toml(&source),
            Err(ThemeError::InsufficientContrast { .. })
        ));
    }

    #[test]
    fn bundled_night_owl_is_valid_dark_and_default() {
        let themes = bundled_themes();
        assert!(
            themes
                .iter()
                .all(|theme| theme.appearance == Appearance::Dark)
        );
        assert!(themes.iter().any(|theme| theme.name == DEFAULT_THEME_NAME));
    }

    #[test]
    fn custom_invalid_theme_is_skipped_and_selection_falls_back() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("bad.toml"), minimal("light")).unwrap();
        let catalog = ThemeCatalog::load_custom(temp.path());
        assert_eq!(catalog.diagnostics.len(), 1);
        let selected = catalog.select_or_default("Missing");
        assert!(selected.used_fallback);
        assert_eq!(selected.theme.name, DEFAULT_THEME_NAME);
    }

    #[test]
    fn configured_custom_theme_loads_from_platform_layout() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::from_root(temp.path());
        fs::create_dir_all(&paths.themes_dir).unwrap();
        fs::write(paths.themes_dir.join("custom.toml"), minimal("dark")).unwrap();
        fs::write(&paths.config_file, "[theme]\nname = \"Test\"\n").unwrap();

        let loaded = LoadedTheme::load(paths);
        assert_eq!(loaded.selected.name, "Test");
        assert!(!loaded.used_fallback);
        assert!(loaded.diagnostics.is_empty());
    }
}
