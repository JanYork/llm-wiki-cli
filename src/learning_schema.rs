pub(crate) fn canonical_tables(plugin: &str) -> Option<&'static [&'static str]> {
    const TUTOR: &[&str] = &[
        "subjects",
        "subject_name_history",
        "requests",
        "tutor_sessions",
        "tutor_diagnoses",
        "tutor_turns",
        "tutor_turn_owner_history",
        "soul_versions",
        "soul_settings",
        "soul_settings_history",
        "soul_proposals",
        "learner_facts",
        "learner_fact_history",
        "tutor_goals",
        "tutor_goal_criteria",
        "tutor_goal_evidence",
        "tutor_goal_history",
        "tutor_plans",
        "tutor_plan_versions",
        "tutor_plan_steps",
        "tutor_plan_step_history",
    ];
    const BOOK: &[&str] = &[
        "subjects",
        "subject_name_history",
        "requests",
        "books",
        "book_blocks",
        "book_anomalies",
        "book_cursors",
        "book_leases",
        "book_lease_owner_history",
        "book_window_reports",
        "book_syntheses",
        "book_summaries",
        "book_mainline",
        "book_relations",
    ];
    const PRACTICE: &[&str] = &[
        "subjects",
        "subject_name_history",
        "requests",
        "practice_banks",
        "practice_items",
        "bank_items",
        "practice_sets",
        "set_members",
        "set_member_events",
        "papers",
        "paper_items",
        "attempts",
        "attempt_takeover_history",
        "responses",
        "response_history",
        "grades",
        "grade_history",
        "review_controls",
        "review_events",
        "fsrs_cards",
        "review_debt",
        "review_debt_events",
    ];
    match plugin {
        "tutor" => Some(TUTOR),
        "book" => Some(BOOK),
        "practice" => Some(PRACTICE),
        _ => None,
    }
}

pub(crate) fn derived_tables(plugin: &str) -> &'static [&'static str] {
    const BOOK: &[&str] = &[
        "book_blocks_fts",
        "book_blocks_fts_data",
        "book_blocks_fts_idx",
        "book_blocks_fts_content",
        "book_blocks_fts_docsize",
        "book_blocks_fts_config",
    ];
    if plugin == "book" { BOOK } else { &[] }
}

pub(crate) fn canonical_logical_hash(
    plugin: &str,
    connection: &Connection,
) -> Result<String, String> {
    let tables = canonical_tables(plugin).ok_or_else(|| "unknown fixed plugin".to_owned())?;
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, plugin.as_bytes());
    for table in tables {
        hash_field(&mut hasher, table.as_bytes());
        let columns = table_columns(connection, table)?;
        if columns.is_empty() {
            return Err(format!(
                "canonical table {table} is missing or has no columns"
            ));
        }
        for column in &columns {
            hash_field(&mut hasher, column.as_bytes());
        }
        let selected = columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {selected} FROM {} ORDER BY {selected}",
            quote_identifier(table)
        );
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            hasher.update([0xff]);
            for index in 0..columns.len() {
                match row.get_ref(index).map_err(|error| error.to_string())? {
                    ValueRef::Null => hasher.update([0]),
                    ValueRef::Integer(value) => {
                        hasher.update([1]);
                        hash_field(&mut hasher, &value.to_be_bytes());
                    }
                    ValueRef::Real(value) => {
                        hasher.update([2]);
                        hash_field(&mut hasher, &value.to_bits().to_be_bytes());
                    }
                    ValueRef::Text(value) => {
                        hasher.update([3]);
                        hash_field(&mut hasher, value);
                    }
                    ValueRef::Blob(value) => {
                        hasher.update([4]);
                        hash_field(&mut hasher, value);
                    }
                }
            }
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))
        .map_err(|error| error.to_string())?;
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}
use rusqlite::{Connection, types::ValueRef};
use sha2::{Digest, Sha256};
