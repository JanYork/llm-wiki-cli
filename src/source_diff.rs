use crate::error::{AppError, Result};
use similar::{Algorithm, TextDiff};
use std::{fmt::Write, time::Duration};

pub const MAX_DIFF_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DIFF_INPUT_LINES: usize = 200_000;
pub const DEFAULT_DIFF_OUTPUT_CHARS: usize = 20_000;
pub const MAX_DIFF_OUTPUT_CHARS: usize = 100_000;

#[derive(Debug, PartialEq, Eq)]
pub struct RenderedDiff {
    pub text: String,
    pub returned_chars: usize,
    pub total_chars: usize,
    pub truncated: bool,
}

pub fn render_diff(
    from: &str,
    to: &str,
    from_label: &str,
    to_label: &str,
    max_chars: usize,
) -> Result<RenderedDiff> {
    if !(1..=MAX_DIFF_OUTPUT_CHARS).contains(&max_chars) {
        return Err(AppError::new(
            "invalid_limit",
            format!("max-chars must be between 1 and {MAX_DIFF_OUTPUT_CHARS}"),
        ));
    }
    validate_input("from", from)?;
    validate_input("to", to)?;
    if from == to {
        return Ok(RenderedDiff {
            text: String::new(),
            returned_chars: 0,
            total_chars: 0,
            truncated: false,
        });
    }

    let mut config = TextDiff::configure();
    config
        .algorithm(Algorithm::Myers)
        .timeout(Duration::from_secs(1));
    let diff = config.diff_lines(from, to);
    let from_label = escape_label(from_label);
    let to_label = escape_label(to_label);
    let complete = diff
        .unified_diff()
        .context_radius(3)
        .header(&from_label, &to_label)
        .to_string();
    let total_chars = complete.chars().count();
    let truncated = total_chars > max_chars;
    let text = if truncated {
        complete.chars().take(max_chars).collect()
    } else {
        complete
    };
    Ok(RenderedDiff {
        text,
        returned_chars: total_chars.min(max_chars),
        total_chars,
        truncated,
    })
}

fn validate_input(side: &str, input: &str) -> Result<()> {
    let bytes = input.len();
    if bytes > MAX_DIFF_INPUT_BYTES {
        return Err(AppError::new(
            "source_diff_too_large",
            format!(
                "{side} input is {bytes} bytes; maximum source diff input is {MAX_DIFF_INPUT_BYTES} bytes"
            ),
        ));
    }
    let lines = input.split_inclusive('\n').count();
    if lines > MAX_DIFF_INPUT_LINES {
        return Err(AppError::new(
            "source_diff_too_large",
            format!(
                "{side} input has {lines} lines; maximum source diff input is {MAX_DIFF_INPUT_LINES} lines"
            ),
        ));
    }
    Ok(())
}

fn escape_label(label: &str) -> String {
    let mut escaped = String::with_capacity(label.len());
    for character in label.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                write!(escaped, "\\u{:04X}", character as u32).unwrap();
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_line_changes_with_safe_headers() {
        let rendered = render_diff(
            "first\nremove me\nport=80\nlast\n",
            "first\nport=81\ninsert me\nlast\n",
            "source:7",
            "live:a\\b\r\n\t\u{7f}中.md",
            MAX_DIFF_OUTPUT_CHARS,
        )
        .unwrap();

        assert!(
            rendered
                .text
                .starts_with("--- source:7\n+++ live:a\\\\b\\r\\n\\t\\u007F中.md\n")
        );
        assert!(rendered.text.contains("-remove me\n"));
        assert!(rendered.text.contains("-port=80\n+port=81\n"));
        assert!(rendered.text.contains("+insert me\n"));
        assert_eq!(rendered.returned_chars, rendered.text.chars().count());
        assert_eq!(rendered.total_chars, rendered.returned_chars);
        assert!(!rendered.truncated);
    }

    #[test]
    fn truncates_only_at_a_unicode_scalar_boundary() {
        let complete = render_diff(
            "甲乙丙丁\n",
            "甲乙😀丁\n",
            "source:1",
            "live:中文.md",
            MAX_DIFF_OUTPUT_CHARS,
        )
        .unwrap();
        assert!(complete.text.contains("-甲乙丙丁\n+甲乙😀丁\n"));
        assert!(!complete.truncated);

        let rendered =
            render_diff("甲乙丙丁\n", "甲乙😀丁\n", "source:1", "live:中文.md", 17).unwrap();

        assert_eq!(rendered.text.chars().count(), 17);
        assert_eq!(rendered.returned_chars, 17);
        assert!(rendered.total_chars > rendered.returned_chars);
        assert!(rendered.truncated);
        assert!(rendered.text.is_char_boundary(rendered.text.len()));
    }

    #[test]
    fn rejects_inputs_above_the_fixed_byte_and_line_caps() {
        let byte_boundary = "x".repeat(MAX_DIFF_INPUT_BYTES);
        assert_eq!(
            render_diff(&byte_boundary, &byte_boundary, "source:1", "source:1", 1)
                .unwrap()
                .text,
            ""
        );
        let oversized = "x".repeat(MAX_DIFF_INPUT_BYTES + 1);
        let error = render_diff(&oversized, "", "source:1", "source:2", 1).unwrap_err();
        assert_eq!(error.code, "source_diff_too_large");

        let line_boundary = "\n".repeat(MAX_DIFF_INPUT_LINES);
        assert_eq!(
            render_diff(&line_boundary, &line_boundary, "source:1", "source:1", 1)
                .unwrap()
                .text,
            ""
        );
        let too_many_lines = "\n".repeat(MAX_DIFF_INPUT_LINES + 1);
        let error = render_diff(&too_many_lines, "", "source:1", "source:2", 1).unwrap_err();
        assert_eq!(error.code, "source_diff_too_large");
    }
}
