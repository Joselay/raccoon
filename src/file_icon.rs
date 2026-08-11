use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconColor {
    Blue,
    Cyan,
    Green,
    Yellow,
    Orange,
    Red,
    Purple,
    Pink,
    Muted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIcon {
    pub glyph: &'static str,
    pub color: IconColor,
}

pub fn for_path(path: &Path) -> FileIcon {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return icon("󰈔", IconColor::Muted);
    };
    let lower_name = file_name.to_ascii_lowercase();

    if let Some(icon) = named_icon(&lower_name) {
        return icon;
    }
    let extension = Path::new(&lower_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    extension_icon(extension)
}

fn named_icon(name: &str) -> Option<FileIcon> {
    Some(match name {
        "cargo.toml" | "cargo.lock" => icon("", IconColor::Orange),
        "package.json" | "package-lock.json" | "npm-shrinkwrap.json" => icon("", IconColor::Red),
        "tsconfig.json" => icon("", IconColor::Blue),
        "deno.json" | "deno.jsonc" => icon("", IconColor::Green),
        "dockerfile"
        | "compose.yaml"
        | "compose.yml"
        | "docker-compose.yaml"
        | "docker-compose.yml" => icon("", IconColor::Blue),
        "makefile" | "gnumakefile" | "justfile" | "taskfile.yml" | "taskfile.yaml" => {
            icon("", IconColor::Yellow)
        }
        "readme" | "readme.md" | "readme.markdown" | "readme.mdx" => icon("󰂺", IconColor::Blue),
        "license" | "license.md" | "license.txt" | "copying" => icon("", IconColor::Yellow),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => {
            icon("", IconColor::Orange)
        }
        ".env" | ".env.local" | ".env.example" | ".editorconfig" => icon("", IconColor::Yellow),
        ".prettierrc" | ".eslintrc" | ".stylelintrc" | ".babelrc" => icon("", IconColor::Purple),
        _ => return None,
    })
}

fn extension_icon(extension: &str) -> FileIcon {
    match extension {
        "rs" => icon("", IconColor::Orange),
        "js" | "mjs" | "cjs" | "jsx" => icon("", IconColor::Yellow),
        "ts" | "mts" | "cts" | "tsx" => icon("", IconColor::Blue),
        "vue" => icon("", IconColor::Green),
        "svelte" => icon("", IconColor::Orange),
        "py" | "pyi" | "pyw" => icon("", IconColor::Yellow),
        "go" => icon("", IconColor::Cyan),
        "java" | "jar" => icon("", IconColor::Red),
        "kt" | "kts" => icon("", IconColor::Purple),
        "rb" | "rake" | "gemspec" => icon("", IconColor::Red),
        "php" => icon("", IconColor::Purple),
        "swift" => icon("", IconColor::Orange),
        "dart" => icon("", IconColor::Blue),
        "ex" | "exs" => icon("", IconColor::Purple),
        "erl" | "hrl" => icon("", IconColor::Red),
        "c" => icon("", IconColor::Blue),
        "h" => icon("", IconColor::Purple),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => icon("", IconColor::Blue),
        "cs" => icon("󰌛", IconColor::Purple),
        "fs" | "fsx" => icon("", IconColor::Blue),
        "lua" => icon("", IconColor::Blue),
        "vim" => icon("", IconColor::Green),
        "sh" | "bash" | "zsh" | "fish" => icon("", IconColor::Green),
        "ps1" => icon("󰨊", IconColor::Blue),
        "html" | "htm" => icon("", IconColor::Orange),
        "css" => icon("", IconColor::Blue),
        "scss" | "sass" => icon("", IconColor::Pink),
        "less" => icon("", IconColor::Blue),
        "md" | "markdown" | "mdx" => icon("󰍔", IconColor::Blue),
        "txt" | "log" => icon("󰈙", IconColor::Muted),
        "toml" => icon("", IconColor::Orange),
        "yaml" | "yml" => icon("", IconColor::Purple),
        "json" | "jsonc" => icon("", IconColor::Yellow),
        "xml" => icon("󰗀", IconColor::Orange),
        "ini" | "conf" | "config" | "properties" => icon("", IconColor::Yellow),
        "sql" | "sqlite" | "db" => icon("", IconColor::Cyan),
        "graphql" | "gql" => icon("󰡷", IconColor::Pink),
        "csv" | "tsv" => icon("", IconColor::Green),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "svg" => {
            icon("󰋩", IconColor::Purple)
        }
        "mp3" | "wav" | "flac" | "m4a" | "ogg" => icon("", IconColor::Pink),
        "mp4" | "mov" | "mkv" | "webm" | "avi" => icon("", IconColor::Pink),
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => icon("", IconColor::Yellow),
        "pdf" => icon("", IconColor::Red),
        "ttf" | "otf" | "woff" | "woff2" => icon("", IconColor::Purple),
        "lock" => icon("", IconColor::Yellow),
        _ => icon("󰈔", IconColor::Muted),
    }
}

const fn icon(glyph: &'static str, color: IconColor) -> FileIcon {
    FileIcon { glyph, color }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_names_take_precedence_over_extensions() {
        assert_eq!(
            for_path(Path::new("Cargo.toml")),
            icon("", IconColor::Orange)
        );
        assert_eq!(
            for_path(Path::new("package.json")),
            icon("", IconColor::Red)
        );
    }

    #[test]
    fn maps_extensions_and_falls_back_safely() {
        assert_eq!(
            for_path(Path::new("src/main.rs")),
            icon("", IconColor::Orange)
        );
        assert_eq!(
            for_path(Path::new("asset.unknown")),
            icon("󰈔", IconColor::Muted)
        );
    }
}
