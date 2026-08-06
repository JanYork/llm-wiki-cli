#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenOccurrence {
    pub normalized: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub ordinal: usize,
}

const STOP_WORDS: &[&str] = &[
    "的", "是", "了", "什么", "在", "有", "和", "与", "对", "从", "the", "is", "a", "an", "what",
    "how", "are", "was", "were", "do", "does", "did", "be", "been", "being", "have", "has", "had",
    "it", "its", "in", "on", "at", "to", "for", "of", "with", "by", "this", "that", "these",
    "those",
];

pub fn tokenize_for_query(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in tokenize(text, false) {
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    tokens
}

#[cfg(test)]
pub fn tokenize_for_index(text: &str) -> Vec<String> {
    tokenize(text, true)
}

pub fn tokenize_for_index_with_positions(text: &str) -> Vec<TokenOccurrence> {
    let mut occurrences = Vec::new();
    tokenize_occurrences_into(text, true, |occurrence| occurrences.push(occurrence));
    occurrences
}

pub fn joined_index_terms(text: &str) -> String {
    let mut joined = String::with_capacity(text.len());
    tokenize_occurrences_into(text, true, |occurrence| {
        joined.push('\u{1e}');
        joined.push_str(&occurrence.normalized);
        joined.push('\u{1f}');
    });
    joined
}

fn tokenize(text: &str, include_cjk_unigrams: bool) -> Vec<String> {
    let mut tokens = Vec::new();
    tokenize_occurrences_into(text, include_cjk_unigrams, |occurrence| {
        tokens.push(occurrence.normalized)
    });
    tokens
}

fn tokenize_occurrences_into(
    text: &str,
    include_cjk_unigrams: bool,
    mut push: impl FnMut(TokenOccurrence),
) {
    let mut word = String::new();
    let mut word_start = None;
    let mut word_end = 0usize;
    let mut cjk: Vec<(char, usize, usize)> = Vec::new();
    let mut ordinal = 0usize;

    for (byte_start, original) in text.char_indices() {
        let byte_end = byte_start + original.len_utf8();
        for ch in original.to_lowercase() {
            if is_cjk(ch) {
                push_word_occurrence(
                    &mut push,
                    &mut ordinal,
                    &mut word,
                    &mut word_start,
                    word_end,
                );
                cjk.push((ch, byte_start, byte_end));
            } else if ch.is_alphanumeric() {
                push_cjk_occurrences(&mut push, &mut ordinal, &mut cjk, include_cjk_unigrams);
                word_start.get_or_insert(byte_start);
                word_end = byte_end;
                word.push(ch);
            } else {
                push_word_occurrence(
                    &mut push,
                    &mut ordinal,
                    &mut word,
                    &mut word_start,
                    word_end,
                );
                push_cjk_occurrences(&mut push, &mut ordinal, &mut cjk, include_cjk_unigrams);
            }
        }
    }
    push_word_occurrence(
        &mut push,
        &mut ordinal,
        &mut word,
        &mut word_start,
        word_end,
    );
    push_cjk_occurrences(&mut push, &mut ordinal, &mut cjk, include_cjk_unigrams);
}

fn push_word_occurrence(
    push: &mut impl FnMut(TokenOccurrence),
    ordinal: &mut usize,
    word: &mut String,
    word_start: &mut Option<usize>,
    word_end: usize,
) {
    let Some(byte_start) = word_start.take() else {
        return;
    };
    let normalized = std::mem::take(word);
    push_occurrence(push, ordinal, normalized, byte_start, word_end);
}

fn push_cjk_occurrences(
    push: &mut impl FnMut(TokenOccurrence),
    ordinal: &mut usize,
    cjk: &mut Vec<(char, usize, usize)>,
    include_unigrams: bool,
) {
    if cjk.len() == 1 {
        let (ch, byte_start, byte_end) = cjk[0];
        push_occurrence(push, ordinal, ch.to_string(), byte_start, byte_end);
    } else {
        for pair in cjk.windows(2) {
            push_occurrence(
                push,
                ordinal,
                [pair[0].0, pair[1].0].iter().collect(),
                pair[0].1,
                pair[1].2,
            );
        }
        if include_unigrams {
            for (ch, byte_start, byte_end) in cjk.iter().copied() {
                push_occurrence(push, ordinal, ch.to_string(), byte_start, byte_end);
            }
        }
    }
    cjk.clear();
}

fn push_occurrence(
    push: &mut impl FnMut(TokenOccurrence),
    ordinal: &mut usize,
    normalized: String,
    byte_start: usize,
    byte_end: usize,
) {
    if is_stop_word(&normalized) {
        return;
    }
    push(TokenOccurrence {
        normalized,
        byte_start,
        byte_end,
        ordinal: *ordinal,
    });
    *ordinal += 1;
}

fn is_stop_word(token: &str) -> bool {
    STOP_WORDS.contains(&token)
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{20000}'..='\u{323af}'
    )
}

#[cfg(test)]
mod tests {
    use super::{
        joined_index_terms, tokenize_for_index, tokenize_for_index_with_positions,
        tokenize_for_query,
    };

    #[test]
    fn chinese_search_tokenizes_meaningful_terms() {
        let tokens = tokenize_for_query("注意力机制是什么？");
        assert!(tokens.contains(&"注意".to_string()));
        assert!(tokens.contains(&"意力".to_string()));
        assert!(tokens.contains(&"机制".to_string()));
        assert!(!tokens.contains(&"什么".to_string()));
        assert!(!tokens.contains(&"是".to_string()));
    }

    #[test]
    fn english_and_mixed_text_are_normalized_consistently() {
        let tokens = tokenize_for_query("Rust tokenizer 和 SQLite search");
        assert!(tokens.contains(&"rust".to_string()));
        assert!(tokens.contains(&"tokenizer".to_string()));
        assert!(tokens.contains(&"sqlite".to_string()));
        assert!(tokens.contains(&"search".to_string()));
        assert!(!tokens.contains(&"Rust".to_string()));
        assert!(!tokens.contains(&"和".to_string()));
    }

    #[test]
    fn punctuation_is_safe_for_cjk_queries() {
        assert_eq!(tokenize_for_query("总资产。"), tokenize_for_query("总资产"));
    }

    #[test]
    fn whitespace_and_punctuation_only_queries_become_empty() {
        assert!(tokenize_for_query("   ").is_empty());
        assert!(tokenize_for_query("，。！？").is_empty());
    }

    #[test]
    fn duplicate_terms_are_removed_deterministically() {
        let tokens = tokenize_for_query("attention attention 注意力 attention");
        assert_eq!(
            tokens
                .iter()
                .filter(|token| token.as_str() == "attention")
                .count(),
            1
        );
        assert_eq!(tokens.first().map(String::as_str), Some("attention"));
        assert!(tokens.contains(&"注意".to_string()));
        assert!(tokens.contains(&"意力".to_string()));
    }

    #[test]
    fn index_and_query_share_normalization_but_not_token_multiplicity() {
        let text = "Rust tokenizer 和 SQLite search";
        let index_tokens = tokenize_for_index(text);
        let query_tokens = tokenize_for_query(text);
        assert!(index_tokens.starts_with(&query_tokens));
        assert_eq!(query_tokens, vec!["rust", "tokenizer", "sqlite", "search"]);
    }

    #[test]
    fn cjk_search_keeps_short_terms_for_recall() {
        let tokens = tokenize_for_query("南京市长江大桥");
        assert!(tokens.contains(&"南京".to_string()));
        assert!(tokens.contains(&"长江".to_string()));
        assert!(tokens.contains(&"大桥".to_string()));
    }

    #[test]
    fn cjk_search_covers_every_adjacent_bigram_without_a_dictionary() {
        let tokens = tokenize_for_query("火星量子鞋");
        assert_eq!(tokens, vec!["火星", "星量", "量子", "子鞋"]);
    }

    #[test]
    fn index_keeps_frequency_while_query_deduplicates() {
        let index_tokens = tokenize_for_index("alpha alpha");
        let query_tokens = tokenize_for_query("alpha alpha");

        assert_eq!(index_tokens, vec!["alpha".to_string(), "alpha".to_string()]);
        assert_eq!(query_tokens, vec!["alpha".to_string()]);
    }

    #[test]
    fn joined_index_terms_are_byte_identical_to_the_token_stream() {
        for text in [
            "Rust tokenizer 和 SQLite search",
            "南京市长江大桥",
            "alpha alpha",
            "注意力\nemoji 🧠 and punctuation!?",
            "什么是 the API_2026 与 Graph-RAG",
        ] {
            let expected = tokenize_for_index(text)
                .into_iter()
                .map(|token| format!("\u{1e}{token}\u{1f}"))
                .collect::<String>();
            assert_eq!(joined_index_terms(text), expected, "input: {text:?}");
        }
    }

    #[test]
    fn positional_tokens_keep_exact_original_utf8_ranges_and_ordinals() {
        let text = "Rust 注意力 alpha";
        let occurrences = tokenize_for_index_with_positions(text);

        assert_eq!(
            occurrences
                .iter()
                .map(|token| (
                    token.normalized.as_str(),
                    &text[token.byte_start..token.byte_end],
                    token.ordinal,
                ))
                .collect::<Vec<_>>(),
            vec![
                ("rust", "Rust", 0),
                ("注意", "注意", 1),
                ("意力", "意力", 2),
                ("注", "注", 3),
                ("意", "意", 4),
                ("力", "力", 5),
                ("alpha", "alpha", 6),
            ]
        );
    }

    #[test]
    fn lowercase_expansion_keeps_original_source_ranges() {
        let text = "İstanbul Σ";

        assert_eq!(
            tokenize_for_index_with_positions(text)
                .iter()
                .map(|token| (
                    token.normalized.as_str(),
                    &text[token.byte_start..token.byte_end],
                ))
                .collect::<Vec<_>>(),
            vec![("i", "İ"), ("stanbul", "stanbul"), ("σ", "Σ")]
        );
        assert_eq!(
            tokenize_for_query(text),
            vec!["i".to_string(), "stanbul".to_string(), "σ".to_string()]
        );
    }
}
