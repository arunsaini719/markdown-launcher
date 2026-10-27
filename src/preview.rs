//! Full-document, read-only Markdown previews with bounded virtual rows.
//! No links, HTML or images are executed.

use std::{ops::Range, path::Path};

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use crate::index::read_markdown_body;

// Limit work for each visible row, not how much of the document can be viewed.
pub const MAX_ROW_CHARS: usize = 1024;
pub const MAX_ROW_NEWLINES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading,
    Code,
    ListItem,
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewBlock {
    pub kind: BlockKind,
    pub text: String,
    pub starts_block: bool,
    pub ends_block: bool,
    pub text_range: Range<usize>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PreviewDocument {
    pub blocks: Vec<PreviewBlock>,
    /// Full rendered text, independent of which virtual rows are mounted.
    pub selection_text: String,
}

impl PreviewDocument {
    /// Runs on the background executor, including the bounded file read and parsing.
    pub fn load(path: &Path, max_file_size_bytes: u64) -> Result<Self, String> {
        let body =
            read_markdown_body(path, max_file_size_bytes).map_err(|error| error.to_string())?;
        Ok(Self::parse(&body))
    }

    fn parse(body: &str) -> Self {
        let mut document = Self::default();
        let mut kind = BlockKind::Paragraph;
        let mut text = String::new();
        for event in Parser::new(body) {
            match event {
                Event::Start(Tag::Heading { .. }) => kind = BlockKind::Heading,
                Event::Start(Tag::CodeBlock(_)) => kind = BlockKind::Code,
                Event::Start(Tag::Item) => {
                    if !text.is_empty() {
                        document.push(kind, &mut text);
                    }
                    kind = BlockKind::ListItem;
                }
                Event::Text(value) | Event::Code(value) => text.push_str(&value),
                Event::SoftBreak | Event::HardBreak => text.push('\n'),
                Event::End(
                    TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item,
                ) => {
                    document.push(kind, &mut text);
                    kind = BlockKind::Paragraph;
                }
                Event::Rule => {
                    document.push(kind, &mut text);
                    document.push(BlockKind::Rule, &mut String::from("────────"));
                }
                // HTML is displayed literally, never interpreted. Images use only
                // their alt text and links only their label: no network/file loads.
                Event::Html(value) | Event::InlineHtml(value) => text.push_str(&value),
                _ => {}
            }
        }
        document.push(kind, &mut text);
        for block in &mut document.blocks {
            if block.kind == BlockKind::ListItem && block.starts_block {
                block.text.insert_str(0, "• ");
            }
            let start = document.selection_text.len();
            document.selection_text.push_str(&block.text);
            block.text_range = start..document.selection_text.len();
            if block.ends_block {
                document.selection_text.push_str("\n\n");
            }
        }
        document.selection_text.truncate(
            document
                .blocks
                .last()
                .map_or(0, |block| block.text_range.end),
        );
        document
    }

    fn push(&mut self, kind: BlockKind, text: &mut String) {
        let text = std::mem::take(text);
        let mut start = 0;
        let mut chars = 0;
        let mut newlines = 0;
        // A single paragraph or fenced code block can span the whole file.
        // Split those too, so virtualization never lays out one enormous row.
        for (offset, character) in text.char_indices() {
            if chars == MAX_ROW_CHARS || newlines == MAX_ROW_NEWLINES {
                self.blocks.push(PreviewBlock {
                    kind,
                    text: text[start..offset].into(),
                    starts_block: start == 0,
                    ends_block: false,
                    text_range: 0..0,
                });
                start = offset;
                chars = 0;
                newlines = 0;
            }
            chars += 1;
            newlines += usize::from(character == '\n');
        }
        if start < text.len() {
            self.blocks.push(PreviewBlock {
                kind,
                text: text[start..].into(),
                starts_block: start == 0,
                ends_block: true,
                text_range: 0..0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_basic_markdown_without_fetching_embedded_resources() {
        let document = PreviewDocument::parse(
            "# Heading\n\nParagraph **bold** and [label](https://example.invalid).\n\n- item\n\n```rust\nlet x = 1;\n```\n\n![alt](https://example.invalid/image.png)",
        );
        assert_eq!(document.blocks[0].kind, BlockKind::Heading);
        assert_eq!(document.blocks[0].text, "Heading");
        assert!(
            document
                .blocks
                .iter()
                .any(|block| block.kind == BlockKind::Code)
        );
        assert!(
            document
                .blocks
                .iter()
                .any(|block| block.kind == BlockKind::ListItem)
        );
        let text = document
            .blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<String>();
        assert!(text.contains("bold and label"));
        assert!(text.contains("alt"));
        assert!(!text.contains("https://"));
    }

    #[test]
    fn retains_all_text_in_unicode_long_lines_and_large_code_blocks() {
        for body in [
            "🦀".repeat(30_000),
            "line\n".repeat(1000),
            "x".repeat(MAX_ROW_CHARS),
            "x".repeat(MAX_ROW_CHARS + 1),
        ] {
            let source = format!("```text\n{body}\n```\n");
            let document = PreviewDocument::parse(&source);
            assert!(document.blocks.iter().all(|block| {
                block.kind == BlockKind::Code
                    && block.text.chars().count() <= MAX_ROW_CHARS
                    && block.text.matches('\n').count() <= MAX_ROW_NEWLINES
            }));
            assert_eq!(
                document
                    .blocks
                    .iter()
                    .map(|block| block.text.as_str())
                    .collect::<String>(),
                format!("{body}\n")
            );
            assert!(document.blocks[0].starts_block);
            assert!(document.blocks.last().expect("last row").ends_block);
            assert_eq!(
                document
                    .blocks
                    .iter()
                    .filter(|block| block.starts_block)
                    .count(),
                1
            );
            assert_eq!(
                document
                    .blocks
                    .iter()
                    .filter(|block| block.ends_block)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn retains_content_past_every_previous_preview_limit() {
        let body = format!(
            "{}\n\n# Final heading\n\nfinal sentinel",
            "paragraph\n\n".repeat(1000)
        );
        let document = PreviewDocument::parse(&body);
        assert_eq!(document.blocks.len(), 1002);
        assert_eq!(document.blocks[1000].text, "Final heading");
        assert_eq!(document.blocks[1001].text, "final sentinel");
        let body = format!("{}tail", "word ".repeat(10_000));
        let document = PreviewDocument::parse(&body);
        assert_eq!(
            document
                .blocks
                .iter()
                .map(|block| block.text.as_str())
                .collect::<String>(),
            body
        );
    }

    #[test]
    fn rejects_invalid_encoding_and_frontmatter() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("note.md");
        std::fs::write(&path, [0xff, 0xfe]).expect("write invalid encoding");
        assert!(PreviewDocument::load(&path, 1024).is_err());
        std::fs::write(&path, "---\ntitle: unclosed\n").expect("write invalid metadata");
        assert!(PreviewDocument::load(&path, 1024).is_err());
    }

    #[test]
    fn reads_current_body_preserving_source_and_enforcing_file_size() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("note.md");
        let source = "---\ntitle: Metadata\n---\n# Current body\n";
        std::fs::write(&path, source).expect("write");
        let preview = PreviewDocument::load(&path, 1024).expect("preview");
        assert_eq!(preview.blocks[0].text, "Current body");
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged file"),
            source
        );
        assert!(PreviewDocument::load(&path, 4).is_err());
        std::fs::write(&path, "replacement").expect("external edit");
        assert_eq!(
            PreviewDocument::load(&path, 1024).expect("reload").blocks[0].text,
            "replacement"
        );
        std::fs::remove_file(&path).expect("remove");
        assert!(PreviewDocument::load(&path, 1024).is_err());
    }
}
