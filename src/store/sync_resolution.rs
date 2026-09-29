fn validate_synthesized_sync_payload(
    tx: &Transaction<'_>,
    kind: &str,
    key: &str,
    conflict: &Value,
    payload: &Value,
) -> Result<()> {
    let identity = match kind {
        "page" => "slug",
        "todo" | "plan" | "memory" | "discussion" => "id",
        _ => {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "this kind does not accept synthesized payloads",
            ));
        }
    };
    ensure_payload_key(payload, identity, key, kind)?;
    let refs = conflict["candidate_refs"].as_array().ok_or_else(|| {
        AppError::new(
            "sync_resolution_invalid",
            "complete candidates are required for a semantic merge",
        )
    })?;
    if refs.is_empty() {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "merge has no candidate evidence",
        ));
    }
    for reference in refs {
        let reference = reference.as_str().ok_or_else(|| {
            AppError::new("sync_resolution_invalid", "invalid candidate reference")
        })?;
        let bytes: Vec<u8> = tx.query_row(
            "SELECT content FROM sync_blobs WHERE content_hash=?1",
            [reference],
            |r| r.get(0),
        )?;
        let original: Value = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::new("sync_resolution_invalid", e.to_string()))?;
        if reference
            != format!(
                "sync-candidate:{}",
                hash_content(&canonical_sync_value(&original))
            )
        {
            return Err(AppError::new(
                "sync_checksum_mismatch",
                "candidate evidence digest mismatch",
            ));
        }
        if let Some(fields) = original.as_object() {
            let submitted = payload.as_object().ok_or_else(|| {
                AppError::new("sync_resolution_invalid", "merge payload must be an object")
            })?;
            if fields.keys().collect::<BTreeSet<_>>() != submitted.keys().collect::<BTreeSet<_>>() {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    "merge must preserve the complete known payload schema",
                ));
            }
        }
        // Local revision/ordinal numbers may be reassigned, but the evidence
        // and original operations that formed either branch cannot disappear.
        if let Some(events) = original["history"].as_array() {
            let submitted = required_array(payload, "history")?
                .iter()
                .map(sync_history_identity)
                .collect::<BTreeSet<_>>();
            if events
                .iter()
                .any(|event| !submitted.contains(&sync_history_identity(event)))
            {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    "merge would discard original history",
                ));
            }
        }
    }
    if kind == "page" {
        for hash in string_array(payload, "source_hashes")? {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_objects WHERE kind='source' AND logical_key=?1)",
                [hash],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    "merge cites a source not present in the fixed input",
                ));
            }
        }
    }
    // The normal publication path independently validates the full domain
    // payload and all cross-object invariants before touching a live Store.
    Ok(())
}

fn sync_history_identity(event: &Value) -> String {
    let mut stable = event.clone();
    if let Some(fields) = stable.as_object_mut() {
        fields.remove("ordinal");
        fields.remove("revision");
    }
    canonical_sync_value(&stable)
}

fn merge_sync_history(left: &[Value], right: &[Value], conflicts: &mut Vec<Value>) -> Value {
    let mut events = BTreeMap::new();
    let mut requests = BTreeMap::new();
    for event in left.iter().chain(right) {
        let identity = sync_history_identity(event);
        if let Some(request) = event["request_id"].as_str()
            && let Some(previous) = requests.insert(request, identity.clone())
                && previous != identity {
                    record_sync_field_conflict(
                        "history",
                        None,
                        &json!(left),
                        &json!(right),
                        conflicts,
                    );
                }
        events.insert(identity, event.clone());
    }
    let mut events: Vec<_> = events.into_iter().collect();
    events.sort_by(|(a, x), (b, y)| {
        x["created_at"]
            .as_str()
            .cmp(&y["created_at"].as_str())
            .then(a.cmp(b))
    });
    Value::Array(
        events
            .into_iter()
            .enumerate()
            .map(|(index, (_, mut event))| {
                if event.get("ordinal").is_some() {
                    event["ordinal"] = json!(index);
                }
                if event.get("revision").is_some() {
                    event["revision"] = json!(index + 1);
                }
                event
            })
            .collect(),
    )
}

pub(crate) fn read_sync_conflict_candidate(path:&Path,reference:&str,offset:u64,limit:u64)->Result<Value> {
    if !reference.strip_prefix("sync-candidate:").is_some_and(|hash|is_lower_sync_hex(hash,64)) || !(1..=16384).contains(&limit) || offset>i64::MAX as u64-1 {
        return Err(AppError::new("invalid_candidate_request","use a packet candidate reference and a 1..16384 character limit"));
    }
    let conn=validate_sync_state_file(path)?;
    let (total,text):(i64,String)=conn.query_row(
        "SELECT length(CAST(content AS TEXT)),substr(CAST(content AS TEXT),?2,?3) FROM sync_blobs WHERE content_hash=?1",
        params![reference,(offset+1) as i64,limit as i64],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let total=u64::try_from(total).map_err(|_|AppError::new("sync_state_invalid","invalid candidate character count"))?;
    let next=offset.saturating_add(text.chars().count() as u64);
    Ok(json!({"reference":reference,"offset":offset,"total_chars":total,"json_fragment":text,"next_offset":(next<total).then_some(next),"complete":offset==0 && next>=total}))
}
