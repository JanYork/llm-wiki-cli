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

// Learning Sync protocol v2 is still pre-release. Its canonical hash deliberately uses this
// typed-row encoding on both SQLite and normalized exports so preserved-only merges never need a
// runtime database. Changing this encoding after v2 publication requires a protocol migration.
pub(crate) fn canonical_logical_hash(
    plugin: &str,
    connection: &Connection,
) -> Result<String, String> {
    let tables = canonical_tables(plugin).ok_or_else(|| "unknown fixed plugin".to_owned())?;
    let mut canonical = empty_canonical_rows(tables);
    for table in tables {
        let mut columns = table_columns(connection, table)?;
        if columns.is_empty() {
            return Err(format!(
                "canonical table {table} is missing or has no columns"
            ));
        }
        columns.sort();
        let selected = columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT {selected} FROM {}", quote_identifier(table));
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let mut encoded = Vec::new();
            for (index, column) in columns.iter().enumerate() {
                encode_sql_value(
                    &mut encoded,
                    column,
                    row.get_ref(index).map_err(|error| error.to_string())?,
                );
            }
            canonical
                .get_mut(*table)
                .expect("fixed table")
                .push(encoded);
        }
    }
    Ok(hash_canonical_rows(plugin, tables, canonical))
}

#[allow(dead_code)] // Core Sync hashes normalized rows; fixed plugin binaries hash SQLite rows.
pub(crate) fn canonical_logical_hash_from_normalized<'a>(
    plugin: &str,
    records: impl Iterator<Item = (&'a str, &'a Map<String, JsonValue>)>,
) -> Result<String, String> {
    let tables = canonical_tables(plugin).ok_or_else(|| "unknown fixed plugin".to_owned())?;
    let mut canonical = empty_canonical_rows(tables);
    for (table, values) in records {
        let rows = canonical
            .get_mut(table)
            .ok_or_else(|| format!("normalized record table {table} is not canonical"))?;
        let mut columns = values.iter().collect::<Vec<_>>();
        columns.sort_by_key(|(column, _)| *column);
        let mut encoded = Vec::new();
        for (column, value) in columns {
            encode_json_value(&mut encoded, column, value)?;
        }
        rows.push(encoded);
    }
    Ok(hash_canonical_rows(plugin, tables, canonical))
}

fn empty_canonical_rows(tables: &[&str]) -> BTreeMap<String, Vec<Vec<u8>>> {
    tables
        .iter()
        .map(|table| ((*table).to_owned(), Vec::new()))
        .collect()
}

fn hash_canonical_rows(
    plugin: &str,
    tables: &[&str],
    mut canonical: BTreeMap<String, Vec<Vec<u8>>>,
) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, plugin.as_bytes());
    for table in tables {
        hash_field(&mut hasher, table.as_bytes());
        let rows = canonical.get_mut(*table).expect("fixed table");
        rows.sort();
        for row in rows {
            hasher.update([0xff]);
            hash_field(&mut hasher, row);
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn encode_sql_value(output: &mut Vec<u8>, column: &str, value: ValueRef<'_>) {
    append_field(output, column.as_bytes());
    match value {
        ValueRef::Null => output.push(0),
        ValueRef::Integer(value) => {
            output.push(1);
            append_field(output, &value.to_be_bytes());
        }
        ValueRef::Real(value) => {
            output.push(2);
            append_field(output, &value.to_bits().to_be_bytes());
        }
        ValueRef::Text(value) => {
            output.push(3);
            append_field(output, value);
        }
        ValueRef::Blob(value) => {
            output.push(4);
            append_field(output, value);
        }
    }
}

#[allow(dead_code)] // Used with normalized rows only in the core Sync binary.
fn encode_json_value(output: &mut Vec<u8>, column: &str, value: &JsonValue) -> Result<(), String> {
    append_field(output, column.as_bytes());
    match value {
        JsonValue::Null => output.push(0),
        JsonValue::Number(value) if value.is_i64() => {
            output.push(1);
            append_field(
                output,
                &value.as_i64().expect("checked integer").to_be_bytes(),
            );
        }
        JsonValue::Number(value) => {
            let value = value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| "normalized record contains an invalid real".to_owned())?;
            output.push(2);
            append_field(output, &value.to_bits().to_be_bytes());
        }
        JsonValue::String(value) => {
            output.push(3);
            append_field(output, value.as_bytes());
        }
        JsonValue::Object(value) if value.len() == 1 && value.contains_key("$blob_hex") => {
            let value = value["$blob_hex"]
                .as_str()
                .ok_or_else(|| "normalized record contains an invalid blob".to_owned())?;
            output.push(4);
            append_field(output, &decode_hex(value)?);
        }
        _ => return Err("normalized record contains an unsupported value".to_owned()),
    }
    Ok(())
}

#[allow(dead_code)] // Used with normalized blob rows only in the core Sync binary.
fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("normalized record contains an invalid blob".to_owned());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| "normalized record contains an invalid blob".to_owned())
        })
        .collect()
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
fn append_field(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}
use rusqlite::{Connection, types::ValueRef};
use serde_json::{Map, Value as JsonValue};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sqlite_and_normalized_hashes_share_one_typed_row_encoding() {
        for plugin in ["tutor", "book", "practice"] {
            let connection = Connection::open_in_memory().unwrap();
            let tables = canonical_tables(plugin).unwrap();
            for (index, table) in tables.iter().enumerate() {
                let schema = if index == 0 {
                    "null_value TEXT,integer_value INTEGER,real_value REAL,text_value TEXT,blob_value BLOB"
                } else {
                    "empty_value TEXT"
                };
                connection
                    .execute_batch(&format!(
                        "CREATE TABLE {}({schema});",
                        quote_identifier(table)
                    ))
                    .unwrap();
            }
            connection
                .execute(
                    &format!(
                        "INSERT INTO {}(null_value,integer_value,real_value,text_value,blob_value)
                         VALUES(NULL,-7,1.25,'typed text',x'00ff')",
                        quote_identifier(tables[0])
                    ),
                    [],
                )
                .unwrap();
            let sqlite = canonical_logical_hash(plugin, &connection).unwrap();
            let values = json!({
                "null_value": null,
                "integer_value": -7,
                "real_value": 1.25,
                "text_value": "typed text",
                "blob_value": {"$blob_hex":"00ff"},
            });
            let values = values.as_object().unwrap();
            let normalized = canonical_logical_hash_from_normalized(
                plugin,
                std::iter::once((tables[0], values)),
            )
            .unwrap();
            assert_eq!(sqlite, normalized, "{plugin}");
        }
    }
}
