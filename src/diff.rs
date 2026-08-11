use std::{
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    FileHeader,
    HunkHeader,
    Addition,
    Deletion,
    Context,
    Metadata,
    Binary,
    Rename,
    NewFile,
    DeletedFile,
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub range: Range<usize>,
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub file_index: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct DiffDocument {
    text: Arc<str>,
    pub lines: Vec<DiffLine>,
    pub files: Vec<Option<PathBuf>>,
}

impl DiffDocument {
    pub fn parse(output: Vec<u8>) -> Self {
        let text: Arc<str> = String::from_utf8_lossy(&output).into_owned().into();
        let mut lines = Vec::with_capacity(text.lines().count());
        let mut offset = 0;
        let mut old = None;
        let mut new = None;
        let mut file_index = None;
        let mut files = Vec::new();

        for raw in text.split_inclusive('\n') {
            let visible = raw
                .strip_suffix('\n')
                .unwrap_or(raw)
                .strip_suffix('\r')
                .unwrap_or(raw.strip_suffix('\n').unwrap_or(raw));
            let start = offset;
            let end = start + visible.len();
            let (kind, old_line, new_line) = if visible.starts_with("diff --git ") {
                files.push(None);
                file_index = Some(files.len() - 1);
                (LineKind::FileHeader, None, None)
            } else if visible.starts_with("+++ ") {
                if let Some(index) = file_index
                    && let Some(path) = parse_file_path(visible, "+++ ", "b/")
                {
                    files[index] = Some(path);
                }
                (LineKind::FileHeader, None, None)
            } else if visible.starts_with("--- ") {
                if let Some(index) = file_index
                    && files[index].is_none()
                    && let Some(path) = parse_file_path(visible, "--- ", "a/")
                {
                    files[index] = Some(path);
                }
                (LineKind::FileHeader, None, None)
            } else if visible.starts_with("index ") {
                (LineKind::FileHeader, None, None)
            } else if visible.starts_with("Binary files ")
                || visible.starts_with("GIT binary patch")
            {
                (LineKind::Binary, None, None)
            } else if visible.starts_with("rename from ")
                || visible.starts_with("rename to ")
                || visible.starts_with("similarity index ")
            {
                (LineKind::Rename, None, None)
            } else if visible.starts_with("new file mode ") {
                (LineKind::NewFile, None, None)
            } else if visible.starts_with("deleted file mode ") {
                (LineKind::DeletedFile, None, None)
            } else if visible.starts_with("\\ No newline at end of file") {
                (LineKind::NoNewline, None, None)
            } else if visible.starts_with("@@") {
                if let Some((old_start, new_start)) = parse_hunk_header(visible) {
                    old = Some(old_start);
                    new = Some(new_start);
                }
                (LineKind::HunkHeader, None, None)
            } else if visible.starts_with('+') {
                let current = new;
                new = new.map(|line| line + 1);
                (LineKind::Addition, None, current)
            } else if visible.starts_with('-') {
                let current = old;
                old = old.map(|line| line + 1);
                (LineKind::Deletion, current, None)
            } else if visible.starts_with(' ') {
                let old_current = old;
                let new_current = new;
                old = old.map(|line| line + 1);
                new = new.map(|line| line + 1);
                (LineKind::Context, old_current, new_current)
            } else {
                (LineKind::Metadata, None, None)
            };
            lines.push(DiffLine {
                range: start..end,
                kind,
                old_line,
                new_line,
                file_index,
            });
            offset += raw.len();
        }

        Self { text, lines, files }
    }

    pub fn line_text(&self, line: &DiffLine) -> &str {
        &self.text[line.range.clone()]
    }

    pub fn text_len(&self) -> usize {
        self.text.len()
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.text.hash(&mut hasher);
        hasher.finish()
    }
}

fn parse_file_path(line: &str, marker: &str, prefix: &str) -> Option<PathBuf> {
    let value = line.strip_prefix(marker)?;
    if value == "/dev/null" {
        return None;
    }
    Some(Path::new(value.strip_prefix(prefix).unwrap_or(value)).to_path_buf())
}

fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    let mut fields = header.split_ascii_whitespace();
    (fields.next()? == "@@").then_some(())?;
    let old = fields
        .next()?
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = fields
        .next()?
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semantics_and_line_numbers() {
        let document = DiffDocument::parse(b"diff --git a/a b/a\n@@ -2,2 +2,2 @@\n-old\n+new\n same\n\\ No newline at end of file\n".to_vec());
        assert_eq!(document.lines[2].kind, LineKind::Deletion);
        assert_eq!(document.lines[2].old_line, Some(2));
        assert_eq!(document.lines[3].new_line, Some(2));
        assert_eq!(document.lines[4].old_line, Some(3));
        assert_eq!(document.line_text(&document.lines[3]), "+new");
    }

    #[test]
    fn identifies_special_file_states() {
        let document = DiffDocument::parse(
            b"diff --git a/old b/new\nsimilarity index 100%\nrename from old\nrename to new\nnew file mode 100644\ndeleted file mode 100644\nBinary files a/image and b/image differ\n\\ No newline at end of file\n"
                .to_vec(),
        );
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Rename)
        );
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.kind == LineKind::NewFile)
        );
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.kind == LineKind::DeletedFile)
        );
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Binary)
        );
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.kind == LineKind::NoNewline)
        );
    }

    #[test]
    fn keeps_a_display_path_for_new_deleted_and_renamed_files() {
        let document = DiffDocument::parse(
            b"diff --git a/old.rs b/old.rs\n--- a/old.rs\n+++ /dev/null\ndiff --git a/dev/null b/new.rs\n--- /dev/null\n+++ b/new.rs\ndiff --git a/before.rs b/after.rs\n--- a/before.rs\n+++ b/after.rs\n"
                .to_vec(),
        );
        assert_eq!(document.files[0].as_deref(), Some(Path::new("old.rs")));
        assert_eq!(document.files[1].as_deref(), Some(Path::new("new.rs")));
        assert_eq!(document.files[2].as_deref(), Some(Path::new("after.rs")));
    }
}
