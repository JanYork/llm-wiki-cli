fn normalize_slug(slug: &str) -> Result<String, Error> {
    let slug = slug.trim().replace('\\', "/");
    if slug.is_empty() || slug.starts_with('/') || slug.len() > REL_PATH_MAX {
        return Err(Error(format!("unsafe page slug: {slug}")));
    }
    let mut parts = Vec::new();
    for part in slug.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(' ')
            || part.ends_with('.')
            || part.chars().count() > SLUG_SEGMENT_MAX
            || part.chars().any(bad_path_char)
        {
            return Err(Error(format!("unsafe page slug: {slug}")));
        }
        parts.push(part.to_string());
    }
    Ok(parts.join("/"))
}

fn validate_raw_ref(path: &str) -> Result<(), Error> {
    let path = path.trim().replace('\\', "/");
    if !(path.starts_with("raw/sources/") || path.starts_with("raw/assets/")) {
        return Err(Error(format!(
            "artifact reference must stay under raw/: {path}"
        )));
    }
    for component in Path::new(&path).components() {
        let Component::Normal(part) = component else {
            return Err(Error(format!("unsafe artifact reference: {path}")));
        };
        let part = part.to_string_lossy();
        if part.is_empty() || part == "." || part == ".." || part.chars().any(bad_path_char) {
            return Err(Error(format!("unsafe artifact reference: {path}")));
        }
    }
    Ok(())
}

fn normalize_source_id(id: &str) -> Result<String, Error> {
    let id = id.trim();
    if id.is_empty() {
        return Err(Error("source id must not be empty".into()));
    }
    let mut out = String::new();
    let mut dashed = false;
    for ch in id.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            dashed = false;
            Some(ch.to_ascii_lowercase())
        } else if matches!(ch, '-' | '_') {
            dashed = false;
            Some(ch)
        } else if !dashed {
            dashed = true;
            Some('-')
        } else {
            None
        };
        if let Some(ch) = next {
            out.push(ch);
            if out.chars().count() >= SOURCE_ID_MAX {
                break;
            }
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        Err(Error(format!(
            "source id cannot be normalized safely: {id}"
        )))
    } else {
        Ok(out)
    }
}

fn safe_basename(origin: &str) -> String {
    let raw = origin
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("source.txt");
    let mut out = String::new();
    let mut dashed = false;
    for ch in raw.chars() {
        if out.chars().count() >= BASENAME_MAX {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
            dashed = false;
        } else if !dashed {
            out.push('-');
            dashed = true;
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();
    if out.is_empty() {
        "source.txt".into()
    } else if out.contains('.') {
        out
    } else {
        format!("{out}.txt")
    }
}

fn normalize_kind(kind: Option<&str>) -> String {
    let kind = kind.unwrap_or("other").trim().to_ascii_lowercase();
    if kind.is_empty() {
        "other".into()
    } else {
        kind
    }
}

fn normalize_provenance(values: &[String]) -> Result<Vec<String>, Error> {
    let values = values.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if let Some(value) = values
        .iter()
        .find(|value| !PROVENANCE_ORDER.contains(value))
    {
        return Err(Error(format!("unsupported page provenance: {value}")));
    }
    Ok(PROVENANCE_ORDER
        .iter()
        .filter(|value| values.contains(**value))
        .map(|value| (*value).to_string())
        .collect())
}

fn folder_for(kind: Option<&str>) -> &'static str {
    match normalize_kind(kind).as_str() {
        "entity" | "entities" => "entities",
        "concept" | "concepts" => "concepts",
        "source" | "sources" => "sources",
        "query" | "queries" => "queries",
        "comparison" | "comparisons" => "comparisons",
        "synthesis" => "synthesis",
        _ => "other",
    }
}

fn day(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
    {
        value[..10].to_string()
    } else {
        "undated".into()
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn text(value: &str) -> String {
    let mut value = value.replace("\r\n", "\n").replace('\r', "\n");
    if !value.is_empty() && !value.ends_with('\n') {
        value.push('\n');
    }
    value
}

fn yaml(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn wiki_label(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '[' | ']' | '|' | '\n' | '\r' | '\t' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bad_path_char(ch: char) -> bool {
    ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*')
}

fn obsidian_app_json() -> &'static str {
    "{\n  \"attachmentFolderPath\": \"raw/assets\",\n  \"userIgnoreFilters\": [\n    \".lwc\"\n  ],\n  \"useMarkdownLinks\": false,\n  \"newLinkFormat\": \"shortest\",\n  \"showUnsupportedFiles\": false\n}\n"
}
