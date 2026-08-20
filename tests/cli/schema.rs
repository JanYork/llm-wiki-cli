fn drop_temporal_schema(conn: &Connection) {
    conn.execute_batch(
        "DROP TABLE IF EXISTS memory_fts;
         DROP TABLE IF EXISTS memory_fts_data;
         DROP TABLE IF EXISTS memory_fts_idx;
         DROP TABLE IF EXISTS memory_fts_content;
         DROP TABLE IF EXISTS memory_fts_docsize;
         DROP TABLE IF EXISTS memory_fts_config;
         DROP TABLE IF EXISTS memory_feedback;
         DROP TABLE IF EXISTS memory_relations;
         DROP TABLE IF EXISTS memory_evidence;
         DROP TABLE IF EXISTS memory_changes;
         DROP TABLE IF EXISTS memory_fragments;
         DROP TABLE IF EXISTS memory_hint_state;
         DROP TABLE IF EXISTS memory_state;
         DROP TABLE IF EXISTS memory_events;",
    )
    .unwrap();
}
