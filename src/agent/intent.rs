const MAX_PROMPT_CHARS: usize = 4_096;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProjectEvidence {
    pub(crate) has_code: bool,
    pub(crate) has_documents: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum MemoryIntent {
    #[default]
    None,
    Recall,
    Record,
    Status,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum BookFormatEvidence {
    #[default]
    None,
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct IntentSet {
    pub(crate) plan: bool,
    pub(crate) todo: bool,
    pub(crate) work: bool,
    pub(crate) sync: bool,
    pub(crate) ingest: bool,
    pub(crate) changeset: bool,
    pub(crate) memory: bool,
    pub(crate) wiki: bool,
    pub(crate) document_graph: bool,
    pub(crate) code_graph: bool,
    pub(crate) tutor: bool,
    pub(crate) practice: bool,
    pub(crate) book: bool,
    pub(crate) office: bool,
    pub(crate) trans: bool,
    pub(crate) memory_intent: MemoryIntent,
    pub(crate) book_format: BookFormatEvidence,
}

pub(crate) fn classify(prompt: &str, evidence: ProjectEvidence) -> IntentSet {
    let text = prompt
        .chars()
        .take(MAX_PROMPT_CHARS)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let words = ascii_words(&text);

    let plan = has_word(&words, &["plan", "roadmap", "planning"])
        || has_zh(
            &text,
            &[
                "当前计划",
                "执行计划",
                "制定计划",
                "更新计划",
                "继续计划",
                "创建计划",
                "计划下一步",
            ],
        );
    let todo = has_word(&words, &["todo"])
        || has_any_phrase(
            &words,
            &[
                &["task", "list"],
                &["to", "do", "list"],
                &["add", "a", "todo"],
            ],
        )
        || has_zh(&text, &["待办", "任务清单"]);
    let work = has_any_phrase(
        &words,
        &[
            &["background", "work"],
            &["work", "item"],
            &["long", "running", "work"],
            &["job", "status"],
            &["running", "job"],
            &["resume", "work"],
            &["lwc", "work"],
        ],
    ) || has_zh(
        &text,
        &["后台任务", "长任务", "工作项", "工作状态", "异步任务"],
    );
    let sync = has_word(&words, &["sync", "synchronize", "synchronise"])
        || has_zh(
            &text,
            &["项目同步", "同步项目", "同步到", "跨机器同步", "恢复同步"],
        );
    let ingest = has_word(&words, &["ingest", "ingestion"])
        || has_any_phrase(
            &words,
            &[
                &["import", "source"],
                &["import", "sources"],
                &["add", "source"],
                &["add", "sources"],
                &["source", "import"],
                &["source", "ingestion"],
                &["integrate", "source"],
            ],
        )
        || has_zh(
            &text,
            &[
                "导入资料",
                "导入来源",
                "资料入库",
                "知识入库",
                "摄取来源",
                "整合来源",
            ],
        );
    let changeset = has_word(&words, &["changeset"])
        || has_any_phrase(&words, &[&["change", "set"], &["draft", "changes"]])
        || has_zh(&text, &["变更集", "草稿变更", "原子变更"]);
    let memory_context = has_any_phrase(
        &words,
        &[
            &["lwc", "memory"],
            &["durable", "memory"],
            &["persistent", "memory"],
            &["project", "memory"],
        ],
    ) || has_zh(&text, &["持久记忆", "长期记忆", "项目记忆"]);
    let memory_recall = has_any_phrase(
        &words,
        &[
            &["memory", "recall"],
            &["recall", "our"],
            &["recall", "from", "memory"],
            &["search", "memory"],
        ],
    ) || has_zh(
        &text,
        &["回忆我们", "回忆项目记忆", "回忆持久记忆", "从记忆中回忆"],
    );
    let memory_record = has_any_phrase(
        &words,
        &[
            &["remember", "this"],
            &["remember", "that"],
            &["remember", "decision"],
            &["save", "to", "memory"],
            &["record", "in", "memory"],
            &["store", "in", "memory"],
        ],
    ) || (memory_context
        && has_word(&words, &["save", "record", "store"])
        && has_word(&words, &["to", "in"]))
        || has_zh(
            &text,
            &[
                "保存到记忆",
                "保存到持久记忆",
                "记住这个",
                "记住这项",
                "记录到记忆",
            ],
        );
    let memory_status = has_any_phrase(
        &words,
        &[
            &["memory", "status"],
            &["lwc", "memory", "pressure"],
            &["durable", "memory", "pressure"],
            &["persistent", "memory", "pressure"],
            &["project", "memory", "pressure"],
            &["memory", "maintenance"],
            &["maintain", "lwc", "memory"],
            &["maintain", "durable", "memory"],
            &["maintain", "persistent", "memory"],
            &["maintain", "project", "memory"],
        ],
    ) || has_zh(
        &text,
        &[
            "记忆状态",
            "记忆压力",
            "维护持久记忆",
            "维护长期记忆",
            "维护项目记忆",
        ],
    );
    let memory_intent = match (memory_recall, memory_record, memory_status) {
        (true, false, false) => MemoryIntent::Recall,
        (false, true, false) => MemoryIntent::Record,
        (false, false, true) => MemoryIntent::Status,
        _ => MemoryIntent::None,
    };
    let memory = memory_context || memory_recall || memory_record || memory_status;
    let wiki = has_word(&words, &["wiki"])
        || has_any_phrase(&words, &[&["knowledge", "base"], &["project", "knowledge"]])
        || has_zh(&text, &["项目维基", "维基页面", "知识库", "项目知识"]);

    let document_graph_intent = has_any_phrase(
        &words,
        &[
            &["document", "graph"],
            &["knowledge", "graph"],
            &["wiki", "graph"],
            &["document", "relationships"],
            &["relationships", "between", "documents"],
            &["citation", "graph"],
            &["linked", "documents"],
            &["document", "neighbors"],
            &["document", "impact"],
        ],
    ) || has_zh(
        &text,
        &[
            "文档图",
            "知识图谱",
            "文档关系",
            "页面关系",
            "引用关系",
            "维基链接",
            "文档关联",
            "页面关联",
        ],
    );
    let code_graph_intent = has_any_phrase(
        &words,
        &[
            &["code", "graph"],
            &["call", "graph"],
            &["code", "structure"],
            &["symbol", "references"],
            &["find", "references"],
            &["dependency", "graph"],
            &["class", "hierarchy"],
            &["function", "callers"],
            &["code", "impact"],
        ],
    ) || has_zh(
        &text,
        &[
            "代码图",
            "代码结构",
            "调用图",
            "调用链",
            "符号引用",
            "查找引用",
            "代码依赖图",
            "类层次",
            "函数调用者",
            "代码影响分析",
        ],
    );

    let tutor = has_word(&words, &["tutor"])
        || has_any_phrase(
            &words,
            &[
                &["teach", "me"],
                &["start", "a", "lesson"],
                &["start", "lesson"],
                &["learning", "session"],
            ],
        )
        || has_zh(&text, &["辅导我", "请教我", "教我", "教学会话", "开始学习"]);
    let practice = has_any_phrase(
        &words,
        &[
            &["start", "practice"],
            &["begin", "practice"],
            &["practice", "session"],
            &["practice", "questions"],
            &["quiz", "me"],
            &["flash", "cards"],
            &["spaced", "repetition"],
            &["grade", "my", "answer"],
            &["review", "schedule"],
            &["practice", "paper"],
            &["mock", "exam"],
        ],
    ) || has_word(&words, &["flashcards"])
        || has_zh(
            &text,
            &[
                "开始练习",
                "练习题",
                "测验我",
                "抽认卡",
                "间隔复习",
                "练习记录",
                "答题会话",
                "模拟试卷",
            ],
        );
    let supported_book_file = has_file_extension(&text, &["epub", "txt", "md", "markdown", "pdf"]);
    let unsupported_book_file = has_file_extension(&text, &["html", "htm", "mobi", "azw3"]);
    let book_action = has_word(&words, &["read", "open", "import", "study", "finish"]);
    let book_phrase = has_any_phrase(
        &words,
        &[
            &["read", "this", "book"],
            &["read", "the", "book"],
            &["whole", "book"],
            &["entire", "book"],
            &["book", "reading"],
            &["import", "a", "book"],
            &["import", "this", "book"],
            &["finish", "this", "book"],
            &["study", "this", "book"],
        ],
    ) || (has_word(&words, &["book"]) && book_action)
        || has_zh(
            &text,
            &[
                "读这本书",
                "看这本书",
                "阅读整本书",
                "导入书籍",
                "整本阅读",
                "读完这本书",
            ],
        );
    let book_only_file = has_file_extension(&text, &["epub", "mobi", "azw3"]);
    let book = book_phrase || (book_only_file && book_action);
    let book_format = if !book {
        BookFormatEvidence::None
    } else if unsupported_book_file {
        BookFormatEvidence::Unsupported
    } else if supported_book_file {
        BookFormatEvidence::Supported
    } else {
        BookFormatEvidence::None
    };

    let office_file = has_file_extension(&text, &["docx", "xlsx", "pptx"]);
    let document_file = has_file_extension(
        &text,
        &[
            "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "html",
            "htm", "epub", "mobi", "txt", "md",
        ],
    );
    let file_use = has_word(
        &words,
        &[
            "open",
            "read",
            "inspect",
            "edit",
            "review",
            "summarize",
            "summarise",
            "analyze",
            "analyse",
            "parse",
            "use",
            "convert",
            "extract",
        ],
    ) || has_zh(
        &text,
        &[
            "打开", "读取", "阅读", "编辑", "查看", "分析", "总结", "解析", "处理", "使用", "转换",
            "转成", "转为", "提取",
        ],
    );
    let conversion = has_word(&words, &["convert", "conversion"])
        || has_any_phrase(
            &words,
            &[
                &["extract", "text"],
                &["extract", "content"],
                &["to", "markdown"],
                &["into", "markdown"],
                &["as", "markdown"],
                &["export", "as", "markdown"],
            ],
        )
        || has_zh(
            &text,
            &[
                "转换文件",
                "转成 markdown",
                "转为 markdown",
                "提取文本",
                "提取内容",
                "导出为 markdown",
            ],
        );

    IntentSet {
        plan,
        todo,
        work,
        sync,
        ingest,
        changeset,
        memory,
        wiki,
        document_graph: evidence.has_documents && document_graph_intent,
        code_graph: evidence.has_code && code_graph_intent,
        tutor,
        practice,
        book,
        office: office_file && file_use,
        trans: document_file && conversion,
        memory_intent,
        book_format,
    }
}

fn ascii_words(text: &str) -> Vec<&str> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect()
}

fn has_word(words: &[&str], candidates: &[&str]) -> bool {
    words.iter().any(|word| candidates.contains(word))
}

fn has_any_phrase(words: &[&str], candidates: &[&[&str]]) -> bool {
    candidates.iter().any(|phrase| {
        !phrase.is_empty() && words.windows(phrase.len()).any(|window| window == *phrase)
    })
}

fn has_zh(text: &str, candidates: &[&str]) -> bool {
    candidates.iter().any(|candidate| text.contains(candidate))
}

fn has_file_extension(text: &str, extensions: &[&str]) -> bool {
    extensions.iter().any(|extension| {
        let needle = format!(".{extension}");
        text.match_indices(&needle).any(|(index, _)| {
            let before = &text[..index];
            let after = &text[index + needle.len()..];
            before.chars().next_back().is_some_and(|character| {
                character.is_alphanumeric() || matches!(character, '_' | '-')
            }) && extension_boundary(after)
        })
    })
}

fn extension_boundary(after: &str) -> bool {
    let mut characters = after.chars();
    match characters.next() {
        None => true,
        Some('.') => characters
            .next()
            .is_none_or(|character| !is_filename_continuation(character)),
        Some(character) => !is_filename_continuation(character),
    }
}

fn is_filename_continuation(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(has_code: bool, has_documents: bool) -> ProjectEvidence {
        ProjectEvidence {
            has_code,
            has_documents,
        }
    }

    fn enabled(intent: &IntentSet) -> Vec<&'static str> {
        let mut names = Vec::new();
        for (name, value) in [
            ("plan", intent.plan),
            ("todo", intent.todo),
            ("work", intent.work),
            ("sync", intent.sync),
            ("ingest", intent.ingest),
            ("changeset", intent.changeset),
            ("memory", intent.memory),
            ("wiki", intent.wiki),
            ("document_graph", intent.document_graph),
            ("code_graph", intent.code_graph),
            ("tutor", intent.tutor),
            ("practice", intent.practice),
            ("book", intent.book),
            ("office", intent.office),
            ("trans", intent.trans),
        ] {
            if value {
                names.push(name);
            }
        }
        names
    }

    #[test]
    fn classifies_each_english_intent_without_broad_keyword_spillover() {
        let fixtures = [
            ("Resume the durable plan.", "plan"),
            ("Add this to my todo list.", "todo"),
            ("Check the background work item status.", "work"),
            ("Sync this project with my laptop.", "sync"),
            ("Ingest these source documents.", "ingest"),
            ("Resume the changeset draft.", "changeset"),
            ("Recall our durable memory.", "memory"),
            ("Update the project wiki.", "wiki"),
            ("Teach me this topic as a tutor.", "tutor"),
            ("Start a practice session with a quiz.", "practice"),
            ("Read this whole book with me.", "book"),
            ("Open quarterly-report.XLSX and summarize it.", "office"),
            ("Convert handbook.pdf to Markdown.", "trans"),
        ];

        for (prompt, expected) in fixtures {
            assert_eq!(
                enabled(&classify(prompt, evidence(false, false))),
                vec![expected],
                "prompt: {prompt}"
            );
        }
    }

    #[test]
    fn classifies_each_chinese_intent_without_returning_prompt_text() {
        let fixtures = [
            ("继续当前计划", "plan"),
            ("把这个加入待办", "todo"),
            ("检查后台任务状态", "work"),
            ("把项目同步到另一台机器", "sync"),
            ("导入这批资料入库", "ingest"),
            ("恢复这个变更集草稿", "changeset"),
            ("把这项决定保存到持久记忆", "memory"),
            ("更新项目知识库", "wiki"),
            ("请教我这个主题", "tutor"),
            ("开始一次练习题测验", "practice"),
            ("陪我阅读整本书", "book"),
            ("打开 季报.xlsx 并总结", "office"),
            ("把 handbook.pdf 转成 Markdown", "trans"),
        ];

        for (prompt, expected) in fixtures {
            let intent = classify(prompt, evidence(false, false));
            assert_eq!(enabled(&intent), vec![expected], "prompt: {prompt}");
            assert!(!format!("{intent:?}").contains(prompt));
        }
    }

    #[test]
    fn graph_applicability_is_independent_and_requires_project_evidence() {
        let document_prompt = "Trace relationships between documents in the knowledge graph.";
        let code_prompt = "Inspect the call graph and code structure.";

        assert!(!classify(document_prompt, evidence(false, false)).document_graph);
        assert!(classify(document_prompt, evidence(false, true)).document_graph);
        assert!(!classify(document_prompt, evidence(true, false)).document_graph);

        assert!(!classify(code_prompt, evidence(false, false)).code_graph);
        assert!(classify(code_prompt, evidence(true, false)).code_graph);
        assert!(!classify(code_prompt, evidence(false, true)).code_graph);

        let both = classify("分析文档关系和代码调用图", evidence(true, true));
        assert!(both.document_graph);
        assert!(both.code_graph);
    }

    #[test]
    fn learning_intent_never_implies_code_graph() {
        for prompt in [
            "Tutor me in Rust.",
            "Start a practice session for coding.",
            "Read this whole programming book.",
            "教我 Rust",
            "开始一次编程练习题测验",
            "陪我阅读整本编程书",
        ] {
            let intent = classify(prompt, evidence(true, false));
            assert!(
                !intent.code_graph,
                "learning leaked into code graph: {prompt}"
            );
        }

        let explicit = classify(
            "Tutor me while we inspect the call graph.",
            evidence(true, false),
        );
        assert!(explicit.tutor);
        assert!(explicit.code_graph);
    }

    #[test]
    fn rejects_english_and_chinese_near_matches() {
        for prompt in [
            "The planner module has a memory allocation bug.",
            "Book a flight and reserve an office.",
            "I practice coding every day.",
            "Wikipedia describes asynchronous workers.",
            "Translate this sentence to Chinese.",
            "What does the .docx extension mean?",
            "Review report.pdf without converting it.",
            "Extract a function from main.rs.",
            "计划器模块存在内存分配问题",
            "预订一张机票并找个办公室",
            "我每天练习编程",
            "翻译这句话，不处理文件",
        ] {
            assert!(
                enabled(&classify(prompt, evidence(true, true))).is_empty(),
                "false positive: {prompt}"
            );
        }
    }

    #[test]
    fn office_and_trans_require_real_file_evidence_and_the_right_action() {
        let office = classify("Please read reports/Q4.PPTX.", evidence(false, true));
        assert!(office.office);
        assert!(!office.trans);

        let trans = classify(
            "Extract text from docs/scanned-report.PDF.",
            evidence(false, true),
        );
        assert!(trans.trans);
        assert!(!trans.office);

        for prompt in [
            "Open a spreadsheet for me.",
            "Please read .xlsx.",
            "Please read report.xlsxx.",
            "Convert this paragraph to Markdown.",
            "The file is report.pdf.",
        ] {
            let intent = classify(prompt, evidence(false, true));
            assert!(!intent.office, "office false positive: {prompt}");
            assert!(!intent.trans, "trans false positive: {prompt}");
        }
    }

    #[test]
    fn supports_multiple_explicit_intents_but_processes_at_most_4096_characters() {
        let combined = classify(
            "Update the wiki, sync the project, then resume the plan.",
            evidence(false, true),
        );
        assert_eq!(enabled(&combined), vec!["plan", "sync", "wiki"]);

        let after_limit = format!("{} plan", "x".repeat(4096));
        assert!(!classify(&after_limit, evidence(false, false)).plan);
        let before_limit = format!("plan {}", "x".repeat(4096));
        assert!(classify(&before_limit, evidence(false, false)).plan);
    }

    #[test]
    fn output_contains_only_typed_flags_not_prompt_fragments() {
        let secret = "PRIVATE_PROMPT_FRAGMENT_9274";
        let intent = classify(
            &format!("Remember this decision in durable memory: {secret}"),
            evidence(false, false),
        );
        assert!(intent.memory);
        assert!(!format!("{intent:?}").contains(secret));
    }

    #[test]
    fn memory_intent_is_typed_by_explicit_english_action() {
        for (prompt, expected) in [
            ("Recall our durable memory.", MemoryIntent::Recall),
            ("Remember this decision for later.", MemoryIntent::Record),
            ("Save this to persistent memory.", MemoryIntent::Record),
            ("Show LWC memory status.", MemoryIntent::Status),
            ("Check durable memory pressure.", MemoryIntent::Status),
            ("Maintain project memory.", MemoryIntent::Status),
            ("Use durable memory for this project.", MemoryIntent::None),
            (
                "Recall our memory and remember this result.",
                MemoryIntent::None,
            ),
        ] {
            let intent = classify(prompt, evidence(false, false));
            assert!(intent.memory, "memory feature missed: {prompt}");
            assert_eq!(intent.memory_intent, expected, "prompt: {prompt}");
        }
    }

    #[test]
    fn memory_intent_is_typed_by_explicit_chinese_action() {
        for (prompt, expected) in [
            ("回忆我们的项目记忆", MemoryIntent::Recall),
            ("把这项决定保存到记忆", MemoryIntent::Record),
            ("记住这个结论", MemoryIntent::Record),
            ("检查持久记忆状态", MemoryIntent::Status),
            ("查看项目记忆压力", MemoryIntent::Status),
            ("维护长期记忆", MemoryIntent::Status),
            ("使用持久记忆", MemoryIntent::None),
            ("回忆项目记忆并记住这个结论", MemoryIntent::None),
        ] {
            let intent = classify(prompt, evidence(false, false));
            assert!(intent.memory, "memory feature missed: {prompt}");
            assert_eq!(intent.memory_intent, expected, "prompt: {prompt}");
        }
    }

    #[test]
    fn book_format_evidence_is_typed_without_retaining_the_filename() {
        for prompt in [
            "Read this book novel.epub.",
            "Read this book novel.txt.",
            "Read this book novel.md.",
            "Read this book novel.markdown.",
            "Read this book novel.pdf.",
            "Read this scanned book scan.pdf.",
        ] {
            let intent = classify(prompt, evidence(false, true));
            assert!(intent.book, "book feature missed: {prompt}");
            assert_eq!(
                intent.book_format,
                BookFormatEvidence::Supported,
                "{prompt}"
            );
        }
        for prompt in [
            "Read this book novel.html.",
            "Read this book novel.htm.",
            "Read this book novel.mobi.",
            "Read this book novel.azw3.",
        ] {
            let intent = classify(prompt, evidence(false, true));
            assert!(intent.book, "book feature missed: {prompt}");
            assert_eq!(
                intent.book_format,
                BookFormatEvidence::Unsupported,
                "{prompt}"
            );
        }

        let no_file = classify("Read this whole book with me.", evidence(false, true));
        assert_eq!(no_file.book_format, BookFormatEvidence::None);
        let not_a_book = classify("Read quarterly-report.pdf.", evidence(false, true));
        assert!(!not_a_book.book);
        assert_eq!(not_a_book.book_format, BookFormatEvidence::None);
    }

    #[test]
    fn unsupported_book_evidence_wins_without_leaking_multi_intent_input() {
        let secret = "PRIVATE_READING_LIST_7391";
        let intent = classify(
            &format!(
                "Sync the project, recall our memory, and read this book {secret}.azw3 after draft.epub."
            ),
            evidence(false, true),
        );
        assert!(intent.sync);
        assert!(intent.memory);
        assert_eq!(intent.memory_intent, MemoryIntent::Recall);
        assert!(intent.book);
        assert_eq!(intent.book_format, BookFormatEvidence::Unsupported);
        assert!(!format!("{intent:?}").contains(secret));
    }

    #[test]
    fn typed_evidence_obeys_the_unicode_prompt_cap_and_defaults_to_none() {
        let default = IntentSet::default();
        assert_eq!(default.memory_intent, MemoryIntent::None);
        assert_eq!(default.book_format, BookFormatEvidence::None);

        let after_limit = format!("{} memory recall novel.azw3", "界".repeat(4096));
        let intent = classify(&after_limit, evidence(false, true));
        assert!(!intent.memory);
        assert_eq!(intent.memory_intent, MemoryIntent::None);
        assert!(!intent.book);
        assert_eq!(intent.book_format, BookFormatEvidence::None);

        let before_limit = format!(
            "memory status read this book novel.epub {}",
            "界".repeat(4096)
        );
        let intent = classify(&before_limit, evidence(false, true));
        assert_eq!(intent.memory_intent, MemoryIntent::Status);
        assert_eq!(intent.book_format, BookFormatEvidence::Supported);
    }
}
