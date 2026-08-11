use std::{ops::Range, str::FromStr};

use anyhow::{Context, Result};
use syntect::{
    easy::ScopeRegionIterator,
    highlighting::ScopeSelector,
    parsing::{ParseState, ScopeStack, SyntaxSet},
};

use crate::diff::{DiffDocument, LineKind};

pub const MAX_HIGHLIGHTED_FILE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_HIGHLIGHTED_FILE_LINES: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxToken {
    Comment,
    Keyword,
    String,
    Number,
    Function,
    Type,
    Variable,
    Constant,
    Operator,
    Punctuation,
    Property,
    Tag,
    Attribute,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    /// Byte range within the diff line, including its one-byte diff prefix.
    pub range: Range<usize>,
    pub token: SyntaxToken,
}

#[derive(Debug, Clone, Default)]
pub struct HighlightedDiff {
    pub lines: Vec<Vec<HighlightSpan>>,
    pub skipped_files: usize,
}

pub struct Highlighter {
    syntaxes: SyntaxSet,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            syntaxes: two_face::syntax::extra_newlines(),
        }
    }

    pub fn highlight(&self, document: &DiffDocument) -> Result<HighlightedDiff> {
        highlight_with_set(document, &self.syntaxes)
    }
}

struct FileState {
    old_parse: ParseState,
    old_scope: ScopeStack,
    new_parse: ParseState,
    new_scope: ScopeStack,
    enabled: bool,
}

pub fn highlight(document: &DiffDocument) -> Result<HighlightedDiff> {
    Highlighter::new().highlight(document)
}

fn highlight_with_set(document: &DiffDocument, syntaxes: &SyntaxSet) -> Result<HighlightedDiff> {
    let selectors = Selectors::new();
    let mut file_sizes = vec![(0usize, 0usize); document.files.len()];
    for line in &document.lines {
        if let Some(index) = line.file_index {
            file_sizes[index].0 += line.range.len();
            file_sizes[index].1 += 1;
        }
    }
    let mut states = document
        .files
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let enabled = file_sizes[index].0 <= MAX_HIGHLIGHTED_FILE_BYTES
                && file_sizes[index].1 <= MAX_HIGHLIGHTED_FILE_LINES;
            let syntax = path
                .as_ref()
                .and_then(|path| path.extension())
                .and_then(|extension| extension.to_str())
                .and_then(|extension| syntaxes.find_syntax_by_extension(extension))
                .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
            FileState {
                old_parse: ParseState::new(syntax),
                old_scope: ScopeStack::new(),
                new_parse: ParseState::new(syntax),
                new_scope: ScopeStack::new(),
                enabled,
            }
        })
        .collect::<Vec<_>>();
    let skipped_files = states.iter().filter(|state| !state.enabled).count();
    let mut result = HighlightedDiff {
        lines: vec![Vec::new(); document.lines.len()],
        skipped_files,
    };

    for (line_index, line) in document.lines.iter().enumerate() {
        let Some(file_index) = line.file_index else {
            continue;
        };
        let state = &mut states[file_index];
        if !state.enabled
            || !matches!(
                line.kind,
                LineKind::Addition | LineKind::Deletion | LineKind::Context
            )
        {
            continue;
        }
        let raw = document.line_text(line);
        let content = raw.get(1..).unwrap_or_default();
        match line.kind {
            LineKind::Addition => {
                result.lines[line_index] = highlight_line(
                    content,
                    &mut state.new_parse,
                    &mut state.new_scope,
                    syntaxes,
                    &selectors,
                )?;
            }
            LineKind::Deletion => {
                result.lines[line_index] = highlight_line(
                    content,
                    &mut state.old_parse,
                    &mut state.old_scope,
                    syntaxes,
                    &selectors,
                )?;
            }
            LineKind::Context => {
                result.lines[line_index] = highlight_line(
                    content,
                    &mut state.new_parse,
                    &mut state.new_scope,
                    syntaxes,
                    &selectors,
                )?;
                // Keep the old-file parser synchronized for later deletion lines.
                let _ = highlight_line(
                    content,
                    &mut state.old_parse,
                    &mut state.old_scope,
                    syntaxes,
                    &selectors,
                )?;
            }
            _ => {}
        }
    }
    Ok(result)
}

fn highlight_line(
    content: &str,
    parser: &mut ParseState,
    stack: &mut ScopeStack,
    syntaxes: &SyntaxSet,
    selectors: &Selectors,
) -> Result<Vec<HighlightSpan>> {
    let mut source = String::with_capacity(content.len() + 1);
    source.push_str(content);
    source.push('\n');
    let operations = parser
        .parse_line(&source, syntaxes)
        .context("parse source line for syntax highlighting")?;
    let mut offset = 1usize;
    let mut spans: Vec<HighlightSpan> = Vec::new();
    for (region, operation) in ScopeRegionIterator::new(&operations, &source) {
        stack
            .apply(operation)
            .context("apply syntax scope operation")?;
        let visible_len = region.len().min(content.len().saturating_sub(offset - 1));
        if visible_len == 0 {
            continue;
        }
        if let Some(token) = selectors.token(stack) {
            let range = offset..offset + visible_len;
            if let Some(previous) = spans.last_mut()
                && previous.token == token
                && previous.range.end == range.start
            {
                previous.range.end = range.end;
            } else {
                spans.push(HighlightSpan { range, token });
            }
        }
        offset += visible_len;
    }
    Ok(spans)
}

struct Selectors {
    invalid: ScopeSelector,
    comment: ScopeSelector,
    keyword: ScopeSelector,
    string: ScopeSelector,
    number: ScopeSelector,
    function: ScopeSelector,
    r#type: ScopeSelector,
    constant: ScopeSelector,
    variable: ScopeSelector,
    operator: ScopeSelector,
    punctuation: ScopeSelector,
    property: ScopeSelector,
    tag: ScopeSelector,
    attribute: ScopeSelector,
}

impl Selectors {
    fn new() -> Self {
        fn selector(value: &str) -> ScopeSelector {
            ScopeSelector::from_str(value).expect("static scope selector")
        }
        Self {
            invalid: selector("invalid"),
            comment: selector("comment"),
            keyword: selector("keyword"),
            string: selector("string"),
            number: selector("constant.numeric"),
            function: selector("entity.name.function"),
            r#type: selector("entity.name.type"),
            constant: selector("constant"),
            variable: selector("variable"),
            operator: selector("keyword.operator"),
            punctuation: selector("punctuation"),
            property: selector("variable.other.member"),
            tag: selector("entity.name.tag"),
            attribute: selector("entity.other.attribute-name"),
        }
    }

    fn token(&self, stack: &ScopeStack) -> Option<SyntaxToken> {
        let scopes = stack.as_slice();
        [
            (&self.invalid, SyntaxToken::Invalid),
            (&self.comment, SyntaxToken::Comment),
            (&self.string, SyntaxToken::String),
            (&self.number, SyntaxToken::Number),
            (&self.function, SyntaxToken::Function),
            (&self.r#type, SyntaxToken::Type),
            (&self.property, SyntaxToken::Property),
            (&self.tag, SyntaxToken::Tag),
            (&self.attribute, SyntaxToken::Attribute),
            (&self.operator, SyntaxToken::Operator),
            (&self.keyword, SyntaxToken::Keyword),
            (&self.constant, SyntaxToken::Constant),
            (&self.variable, SyntaxToken::Variable),
            (&self.punctuation, SyntaxToken::Punctuation),
        ]
        .into_iter()
        .find_map(|(selector, token)| selector.does_match(scopes).map(|_| token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_tokens_inside_diff_lines() {
        let document = DiffDocument::parse(
            b"diff --git a/main.rs b/main.rs\n--- a/main.rs\n+++ b/main.rs\n@@ -0,0 +1,2 @@\n+fn main() {\n+    // hello\n"
                .to_vec(),
        );
        let highlighted = highlight(&document).unwrap();
        assert!(
            highlighted.lines[4]
                .iter()
                .any(|span| span.token == SyntaxToken::Function)
        );
        assert!(
            highlighted.lines[5]
                .iter()
                .any(|span| span.token == SyntaxToken::Comment)
        );
    }

    #[test]
    fn skips_files_over_the_size_limit() {
        let mut source = String::from(
            "diff --git a/large.rs b/large.rs\n--- a/large.rs\n+++ b/large.rs\n@@ -0,0 +1,1 @@\n+",
        );
        source.push_str(&"x".repeat(MAX_HIGHLIGHTED_FILE_BYTES + 1));
        source.push('\n');
        let document = DiffDocument::parse(source.into_bytes());
        let highlighted = highlight(&document).unwrap();
        assert_eq!(highlighted.skipped_files, 1);
        assert!(highlighted.lines.iter().all(Vec::is_empty));
    }
}
