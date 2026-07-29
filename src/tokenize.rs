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

pub fn tokenize_for_index(text: &str) -> Vec<String> {
    tokenize(text, true)
}

fn tokenize(text: &str, include_cjk_unigrams: bool) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut cjk = Vec::new();

    for ch in text.to_lowercase().chars() {
        if is_cjk(ch) {
            push_word(&mut tokens, &mut word);
            cjk.push(ch);
        } else if ch.is_alphanumeric() {
            push_cjk(&mut tokens, &mut cjk, include_cjk_unigrams);
            word.push(ch);
        } else {
            push_word(&mut tokens, &mut word);
            push_cjk(&mut tokens, &mut cjk, include_cjk_unigrams);
        }
    }
    push_word(&mut tokens, &mut word);
    push_cjk(&mut tokens, &mut cjk, include_cjk_unigrams);

    tokens
}

fn push_word(tokens: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() && !is_stop_word(word) {
        tokens.push(std::mem::take(word));
    } else {
        word.clear();
    }
}

fn push_cjk(tokens: &mut Vec<String>, cjk: &mut Vec<char>, include_unigrams: bool) {
    if cjk.len() == 1 {
        let token = cjk[0].to_string();
        if !is_stop_word(&token) {
            tokens.push(token);
        }
    } else {
        for pair in cjk.windows(2) {
            let token = pair.iter().collect::<String>();
            if !is_stop_word(&token) {
                tokens.push(token);
            }
        }
        if include_unigrams {
            for ch in cjk.iter() {
                let token = ch.to_string();
                if !is_stop_word(&token) {
                    tokens.push(token);
                }
            }
        }
    }
    cjk.clear();
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
    use super::{tokenize_for_index, tokenize_for_query};

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
}
