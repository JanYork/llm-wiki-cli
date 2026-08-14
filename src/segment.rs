use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub const SEGMENTER_VERSION: u32 = 1;
pub const MAX_PASSAGE_BYTES: usize = 64 * 1024;
pub const MAX_SENTENCE_BYTES: usize = 16 * 1024;
pub const MAX_SPANS_PER_DOCUMENT: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassageKind {
    Paragraph,
    Heading,
    ListItem,
    BlockQuote,
    TableRow,
    CodeBlock,
    Html,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceSpan {
    pub ordinal: usize,
    pub range: Range<usize>,
    pub continued: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassageSpan {
    pub kind: PassageKind,
    pub ordinal: usize,
    pub range: Range<usize>,
    pub sentences: Vec<SentenceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentedDocument {
    pub version: u32,
    pub passages: Vec<PassageSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentError {
    TooManySpans { limit: usize, actual: usize },
    TooManyPositions { limit: usize, actual: usize },
}

pub fn segment_document(content: &str) -> Result<SegmentedDocument, SegmentError> {
    let mut candidates = Vec::new();
    let mut item_depth = 0usize;
    let mut quote_depth = 0usize;
    let mut code_depth = 0usize;
    let options = Options::ENABLE_TABLES;

    for (event, range) in Parser::new_ext(content, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Item) => {
                candidates.push((PassageKind::ListItem, range));
                item_depth += 1;
            }
            Event::Start(Tag::BlockQuote(_)) => {
                candidates.push((PassageKind::BlockQuote, range));
                quote_depth += 1;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                if code_depth == 0 {
                    candidates.push((PassageKind::CodeBlock, range));
                }
                code_depth += 1;
            }
            Event::Start(Tag::Heading { .. }) => {
                candidates.push((PassageKind::Heading, range));
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                candidates.push((PassageKind::TableRow, range));
            }
            Event::Start(Tag::Paragraph)
                if item_depth == 0 && quote_depth == 0 && code_depth == 0 =>
            {
                candidates.push((PassageKind::Paragraph, range));
            }
            Event::Html(_) if item_depth == 0 && quote_depth == 0 && code_depth == 0 => {
                candidates.push((PassageKind::Html, range));
            }
            Event::End(TagEnd::Item) => item_depth = item_depth.saturating_sub(1),
            Event::End(TagEnd::BlockQuote(_)) => quote_depth = quote_depth.saturating_sub(1),
            Event::End(TagEnd::CodeBlock) => code_depth = code_depth.saturating_sub(1),
            _ => {}
        }
    }

    let mut passages = non_overlapping_candidates(content, &candidates)
        .into_iter()
        .flat_map(|(kind, range)| {
            split_utf8_range(content, range, MAX_PASSAGE_BYTES)
                .into_iter()
                .filter_map(move |(range, _)| trim_range(content, range).map(|range| (kind, range)))
        })
        .map(|(kind, range)| PassageSpan {
            kind,
            ordinal: 0,
            sentences: sentence_spans(content, kind, &range),
            range,
        })
        .collect::<Vec<_>>();
    passages.sort_by_key(|passage| (passage.range.start, passage.range.end));
    for (ordinal, passage) in passages.iter_mut().enumerate() {
        passage.ordinal = ordinal;
    }
    let span_count = passages.len()
        + passages
            .iter()
            .map(|passage| passage.sentences.len())
            .sum::<usize>();
    if span_count > MAX_SPANS_PER_DOCUMENT {
        return Err(SegmentError::TooManySpans {
            limit: MAX_SPANS_PER_DOCUMENT,
            actual: span_count,
        });
    }
    Ok(SegmentedDocument {
        version: SEGMENTER_VERSION,
        passages,
    })
}

fn non_overlapping_candidates(
    content: &str,
    candidates: &[(PassageKind, Range<usize>)],
) -> Vec<(PassageKind, Range<usize>)> {
    let mut ordered = candidates.to_vec();
    ordered.sort_by_key(|(_, range)| (range.start, std::cmp::Reverse(range.end)));
    let mut direct_children = vec![Vec::new(); ordered.len()];
    let mut stack: Vec<usize> = Vec::new();
    for index in 0..ordered.len() {
        let range = &ordered[index].1;
        while let Some(parent_index) = stack.last().copied() {
            let parent = &ordered[parent_index].1;
            if parent.start <= range.start
                && range.end <= parent.end
                && (parent.start != range.start || parent.end != range.end)
            {
                direct_children[parent_index].push(range.clone());
                break;
            }
            stack.pop();
        }
        stack.push(index);
    }

    let mut passages = Vec::new();
    for ((kind, parent), children) in ordered.iter().zip(direct_children) {
        let mut cursor = parent.start;
        for child in children {
            if cursor < child.start {
                if let Some(range) = trim_range(content, cursor..child.start) {
                    passages.push((*kind, range));
                }
            }
            cursor = cursor.max(child.end);
        }
        if let Some(range) = trim_range(content, cursor..parent.end) {
            passages.push((*kind, range));
        }
    }
    passages.sort_by_key(|(_, range)| (range.start, range.end));
    passages.dedup_by(|left, right| left.1 == right.1);
    passages
}

fn sentence_spans(content: &str, kind: PassageKind, passage: &Range<usize>) -> Vec<SentenceSpan> {
    let ranges = if kind == PassageKind::CodeBlock {
        code_line_ranges(content, passage)
    } else {
        content[passage.clone()]
            .split_sentence_bound_indices()
            .flat_map(|(offset, sentence)| {
                let range = (passage.start + offset)..(passage.start + offset + sentence.len());
                trim_range(content, range)
                    .map(|range| split_utf8_range(content, range, MAX_SENTENCE_BYTES))
                    .unwrap_or_default()
            })
            .collect()
    };
    ranges
        .into_iter()
        .enumerate()
        .map(|(ordinal, (range, continued))| SentenceSpan {
            ordinal,
            range,
            continued,
        })
        .collect()
}

fn code_line_ranges(content: &str, passage: &Range<usize>) -> Vec<(Range<usize>, bool)> {
    let mut offset = passage.start;
    content[passage.clone()]
        .split_inclusive('\n')
        .flat_map(|line| {
            let range = offset..(offset + line.len());
            offset += line.len();
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                Vec::new()
            } else {
                trim_range(content, range)
                    .map(|range| split_utf8_range(content, range, MAX_SENTENCE_BYTES))
                    .unwrap_or_default()
            }
        })
        .collect()
}

fn split_utf8_range(
    content: &str,
    range: Range<usize>,
    max_bytes: usize,
) -> Vec<(Range<usize>, bool)> {
    let mut chunks = Vec::new();
    let mut start = range.start;
    while range.end - start > max_bytes {
        let mut end = start + max_bytes;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push((start..end, !chunks.is_empty()));
        start = end;
    }
    chunks.push((start..range.end, !chunks.is_empty()));
    chunks
}

fn trim_range(content: &str, range: Range<usize>) -> Option<Range<usize>> {
    let slice = &content[range.clone()];
    let start = slice
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(offset, _)| range.start + offset)?;
    let end = slice
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(offset, ch)| range.start + offset + ch.len_utf8())?;
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::{PassageKind, SEGMENTER_VERSION, segment_document};

    #[test]
    fn segments_heading_and_paragraph_into_exact_unicode_sentences() {
        let content = "# 标题\n\nFirst sentence. Second!\n";

        let document = segment_document(content).unwrap();

        assert_eq!(document.version, SEGMENTER_VERSION);
        assert_eq!(document.passages.len(), 2);
        assert_eq!(document.passages[0].kind, PassageKind::Heading);
        assert_eq!(&content[document.passages[0].range.clone()], "# 标题");
        assert_eq!(document.passages[0].ordinal, 0);
        assert_eq!(document.passages[0].sentences.len(), 1);
        assert_eq!(
            &content[document.passages[0].sentences[0].range.clone()],
            "# 标题"
        );
        assert_eq!(document.passages[1].kind, PassageKind::Paragraph);
        assert_eq!(
            &content[document.passages[1].range.clone()],
            "First sentence. Second!"
        );
        assert_eq!(
            document.passages[1]
                .sentences
                .iter()
                .map(|sentence| &content[sentence.range.clone()])
                .collect::<Vec<_>>(),
            vec!["First sentence.", "Second!"]
        );
    }

    #[test]
    fn preserves_list_quote_table_code_and_html_block_boundaries() {
        let content = concat!(
            "- item one. Next?\n  continued\n\n",
            "> quote here. More.\n\n",
            "| a | b |\n|---|---|\n| c | d |\n\n",
            "```rust\nlet x = 1;\nprintln!(\"{x}\");\n```\n\n",
            "<div>hello</div>\n",
        );

        let document = segment_document(content).unwrap();
        let passages = document
            .passages
            .iter()
            .map(|passage| (passage.kind, &content[passage.range.clone()]))
            .collect::<Vec<_>>();

        assert_eq!(
            passages,
            vec![
                (PassageKind::ListItem, "- item one. Next?\n  continued"),
                (PassageKind::BlockQuote, "> quote here. More."),
                (PassageKind::TableRow, "| a | b |"),
                (PassageKind::TableRow, "| c | d |"),
                (
                    PassageKind::CodeBlock,
                    "```rust\nlet x = 1;\nprintln!(\"{x}\");\n```"
                ),
                (PassageKind::Html, "<div>hello</div>"),
            ]
        );
        let code = &document.passages[4];
        assert_eq!(
            code.sentences
                .iter()
                .map(|sentence| &content[sentence.range.clone()])
                .collect::<Vec<_>>(),
            vec!["let x = 1;", "println!(\"{x}\");"]
        );
    }

    #[test]
    fn segments_adjacent_nested_lists_without_overlapping_range_panic() {
        let content = concat!(
            "* Classic Rock-inspired Indie Bands:\n",
            "\t+ Greta Van Fleet\n",
            "\t+ Rival Sons\n",
            "\t+ The Black Keys\n",
            "\t+ Royal Blood\n",
            "* Indie Bands with a Classic Rock Influence:\n",
            "\t+ Arctic Monkeys\n",
            "\t+ The Strokes\n",
        );

        let document = segment_document(content).unwrap();

        assert!(!document.passages.is_empty());
        assert!(document.passages.iter().all(|passage| {
            passage.range.start <= passage.range.end && passage.range.end <= content.len()
        }));
    }

    #[test]
    fn splits_long_code_lines_at_utf8_boundaries_and_marks_continuations() {
        let line = "界".repeat(super::MAX_SENTENCE_BYTES / "界".len() + 2);
        let content = format!("```text\n{line}\n```\n");

        let document = segment_document(&content).unwrap();
        let sentences = &document.passages[0].sentences;

        assert_eq!(sentences.len(), 2);
        assert!(
            sentences
                .iter()
                .all(|sentence| sentence.range.len() <= super::MAX_SENTENCE_BYTES)
        );
        assert!(!sentences[0].continued);
        assert!(sentences[1].continued);
        assert_eq!(
            sentences
                .iter()
                .map(|sentence| &content[sentence.range.clone()])
                .collect::<String>(),
            line
        );
    }

    #[test]
    fn nested_list_items_remain_separate_non_overlapping_passages() {
        let content = "- outer first.\n  - nested item.\n- second item.\n";

        let document = segment_document(content).unwrap();

        assert_eq!(
            document
                .passages
                .iter()
                .map(|passage| (passage.kind, &content[passage.range.clone()]))
                .collect::<Vec<_>>(),
            vec![
                (PassageKind::ListItem, "- outer first."),
                (PassageKind::ListItem, "- nested item."),
                (PassageKind::ListItem, "- second item."),
            ]
        );
        assert!(document.passages.windows(2).all(|pair| {
            pair[0].range.end <= pair[1].range.start && pair[0].ordinal + 1 == pair[1].ordinal
        }));
    }

    #[test]
    fn rejects_documents_over_the_total_span_limit() {
        let content = "x\n\n".repeat(super::MAX_SPANS_PER_DOCUMENT / 2 + 1);

        let error = segment_document(&content).unwrap_err();

        assert_eq!(
            error,
            super::SegmentError::TooManySpans {
                limit: super::MAX_SPANS_PER_DOCUMENT,
                actual: super::MAX_SPANS_PER_DOCUMENT + 2,
            }
        );
    }

    #[test]
    fn splits_long_prose_sentences_without_breaking_utf8() {
        let sentence = format!("{}。", "知".repeat(super::MAX_SENTENCE_BYTES / 3 + 2));

        let document = segment_document(&sentence).unwrap();
        let spans = &document.passages[0].sentences;

        assert_eq!(spans.len(), 2);
        assert!(!spans[0].continued);
        assert!(spans[1].continued);
        assert!(
            spans
                .iter()
                .all(|span| span.range.len() <= super::MAX_SENTENCE_BYTES)
        );
        assert_eq!(
            spans
                .iter()
                .map(|span| &sentence[span.range.clone()])
                .collect::<String>(),
            sentence
        );
    }

    #[test]
    fn splits_structural_blocks_over_the_passage_limit() {
        let line = format!("{}\n", "x".repeat(15_000));
        let content = format!("```text\n{}\n```\n", line.repeat(5));

        let document = segment_document(&content).unwrap();

        assert!(document.passages.len() > 1);
        assert!(
            document
                .passages
                .iter()
                .all(|passage| passage.kind == PassageKind::CodeBlock
                    && passage.range.len() <= super::MAX_PASSAGE_BYTES)
        );
        assert!(document.passages.windows(2).all(|pair| {
            pair[0].range.end <= pair[1].range.start && pair[0].ordinal + 1 == pair[1].ordinal
        }));
    }

    #[test]
    fn unicode_and_crlf_boundaries_are_exact_and_repeatable() {
        let content = "第一句。第二句！\r\nCafe\u{301} works. Next?\r\n";

        let first = segment_document(content).unwrap();
        let second = segment_document(content).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            first.passages[0]
                .sentences
                .iter()
                .map(|sentence| &content[sentence.range.clone()])
                .collect::<Vec<_>>(),
            vec!["第一句。", "第二句！", "Cafe\u{301} works.", "Next?"]
        );
        assert!(
            first.passages[0]
                .sentences
                .iter()
                .all(|sentence| content.is_char_boundary(sentence.range.start)
                    && content.is_char_boundary(sentence.range.end))
        );
    }

    #[test]
    fn formatting_only_markdown_does_not_create_empty_spans() {
        let document = segment_document("---\n\n***\n").unwrap();

        assert!(document.passages.is_empty());
    }
}
