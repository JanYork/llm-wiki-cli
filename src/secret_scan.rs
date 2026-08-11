use std::path::Path;

pub(crate) fn detect_possible_secret_reasons(path: &Path, content: &str) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let env_file = name == ".env"
        || (name.starts_with(".env.")
            && ![".env.example", ".env.sample", ".env.template"].contains(&name.as_str()));
    let private_file = matches!(
        name.as_str(),
        "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519"
    ) || path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| {
            ["key", "p12", "pfx"]
                .iter()
                .any(|extension| value.eq_ignore_ascii_case(extension))
        })
        .unwrap_or(false);
    if env_file {
        reasons.push("environment credential file");
    }
    if private_file {
        reasons.push("private-key or credential file");
    }
    if [
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "-----BEGIN PGP PRIVATE KEY BLOCK-----",
    ]
    .iter()
    .any(|marker| content.contains(marker))
    {
        reasons.push("private-key marker");
    }
    if [
        ("AKIA", 20),
        ("ASIA", 20),
        ("ghp_", 20),
        ("github_pat_", 24),
        ("sk-proj-", 24),
        ("xoxb-", 24),
        ("xoxp-", 24),
    ]
    .iter()
    .any(|(prefix, minimum)| contains_token(content, prefix, *minimum))
    {
        reasons.push("known credential prefix");
    }
    reasons
}

fn contains_token(content: &str, prefix: &str, minimum_length: usize) -> bool {
    content.match_indices(prefix).any(|(index, _)| {
        content[index..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
            .count()
            >= minimum_length
    })
}
