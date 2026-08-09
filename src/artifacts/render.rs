fn render_page(page: &PlannedPage) -> String {
    let mut out = vec![
        "---".to_string(),
        format!("type: {}", yaml(&page.kind)),
        format!("title: {}", yaml(&page.title)),
        "sources:".to_string(),
    ];
    if page.sources.is_empty() {
        out.push("  []".to_string());
    } else {
        for source in &page.sources {
            out.push(format!("  - {}", yaml(source)));
        }
    }
    out.push("provenance:".to_string());
    if page.provenance.is_empty() {
        out.push("  []".to_string());
    } else {
        for provenance in &page.provenance {
            out.push(format!("  - {}", yaml(provenance)));
        }
    }
    out.push(format!("created: {}", yaml(&page.created)));
    out.push(format!("updated: {}", yaml(&page.updated)));
    if let Some(summary) = &page.summary {
        out.push(format!("summary: {}", yaml(summary)));
    }
    out.push("---".to_string());
    out.push(String::new());
    if !page.body.trim().is_empty() {
        out.push(page.body.trim_end_matches('\n').to_string());
    }
    out.join("\n") + "\n"
}

fn render_index(pages: &[PlannedPage]) -> String {
    let mut groups: BTreeMap<&str, Vec<&PlannedPage>> = BTreeMap::new();
    for page in pages {
        groups.entry(page.folder).or_default().push(page);
    }
    let mut out = String::from("# Wiki Index\n");
    for (key, label) in [
        ("entities", "Entities"),
        ("concepts", "Concepts"),
        ("sources", "Sources"),
        ("queries", "Queries"),
        ("comparisons", "Comparisons"),
        ("synthesis", "Synthesis"),
        ("other", "Other"),
    ] {
        out.push_str(&format!("\n## {label}\n"));
        if let Some(entries) = groups.get_mut(key) {
            entries.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.path.cmp(&b.path)));
            for page in entries {
                let target = page
                    .path
                    .strip_prefix("wiki/")
                    .unwrap_or(&page.path)
                    .strip_suffix(".md")
                    .unwrap_or(&page.path);
                out.push_str(&format!("- [[{}|{}]]", target, wiki_label(&page.title)));
                if let Some(summary) = page.summary.as_deref().and_then(inline_log_value) {
                    out.push_str(&format!(" — {summary}"));
                }
                out.push('\n');
            }
        }
    }
    out
}

fn render_log(operations: &[Operation]) -> String {
    let mut out = String::from("# Wiki Log\n");
    for op in operations {
        let action = inline_log_value(&op.action).unwrap_or_else(|| "unknown".into());
        let target = inline_log_value(&op.target).unwrap_or_else(|| "unknown".into());
        out.push_str(&format!(
            "\n## [{}] {} | {}\n",
            day(&op.created_at),
            action,
            target
        ));
        out.push_str(&format!("Timestamp: {}\n", op.created_at.trim()));
        if let Some(detail) = trimmed(op.detail.as_deref()) {
            out.push_str(&format!("Detail: {}\n", detail));
        }
    }
    out
}

fn inline_log_value(value: &str) -> Option<String> {
    let cleaned = value
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn render_overview(snapshot: &Snapshot, pages: &[PlannedPage]) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    let (mut created, mut updated) = ("unknown", "unknown");
    for (index, page) in pages.iter().enumerate() {
        *counts.entry(page.folder).or_default() += 1;
        if index == 0 || page.created.as_str() < created {
            created = &page.created;
        }
        if index == 0 || page.updated.as_str() > updated {
            updated = &page.updated;
        }
    }
    let mut out = vec![
        "---".to_string(),
        r#"type: "overview""#.to_string(),
        r#"title: "Project Overview""#.to_string(),
        format!("created: {}", yaml(created)),
        format!("updated: {}", yaml(updated)),
        "---".to_string(),
        String::new(),
        "# Overview".to_string(),
        String::new(),
        format!("- Sources: {}", snapshot.sources.len()),
        format!("- Pages: {}", pages.len()),
        format!("- Operations: {}", snapshot.operations.len()),
    ];
    if !counts.is_empty() {
        out.push(String::new());
        out.push("## Page Types".to_string());
        out.push(String::new());
        for (folder, count) in counts {
            out.push(format!("- {}: {}", folder, count));
        }
    }
    let purpose = text(&snapshot.purpose);
    if !purpose.trim().is_empty() {
        out.push(String::new());
        out.push("## Purpose Snapshot".to_string());
        out.push(String::new());
        out.push(purpose.trim_end().to_string());
    }
    out.join("\n") + "\n"
}
