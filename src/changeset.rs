use crate::{
    error::{AppError, Result},
    scope::{Scope, StorePath},
    store::{
        ChangesetDraftState, ChangesetPublishInput, ChangesetRollbackInput,
        ChangesetSyncReplayItemState, DetachedChangesetIntent, Store,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[cfg(test)]
#[path = "sync_continuity_tests.rs"]
mod sync_continuity_tests;

#[cfg(test)]
#[path = "changeset_hook_summary_tests.rs"]
mod hook_summary_tests;

const CHANGESET_HOOK_MAX_ITEMS: usize = 3;
const CHANGESET_HOOK_MAX_SCAN_ITEMS: usize = 64;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ChangesetHookSummary {
    pub(crate) changesets: Vec<ChangesetHookSummaryItem>,
    pub(crate) omitted: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ChangesetHookSummaryItem {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) staged_operation_count: usize,
    pub(crate) empty: bool,
    pub(crate) conflict: bool,
}

#[derive(Debug, Serialize)]
pub struct ChangesetBeginResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: String,
    pub base_revision: String,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct ChangesetShowResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: String,
    pub base_revision: String,
    pub draft_revision: String,
    pub staged_operation_count: usize,
    pub action_counts: std::collections::BTreeMap<String, usize>,
    pub operations: Vec<crate::store::OperationRecord>,
    pub empty: bool,
    pub conflict: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ChangesetListResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changesets: Vec<ChangesetShowResponse>,
}

#[derive(Debug, Serialize)]
pub struct ChangesetDiscardResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ChangesetCommitResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: &'static str,
    pub base_revision: String,
    pub post_revision: String,
    pub checkpoint: String,
    pub staged_operation_count: usize,
    pub lint_issues: usize,
    pub materialized: bool,
    pub wal_checkpointed: bool,
    pub duration_ms: u64,
    pub checkpoint_ms: u64,
    pub locked_publish_ms: u64,
    pub wal_checkpoint_ms: u64,
    pub cleanup_ms: u64,
    pub materialization_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_work: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ChangesetRollbackResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: &'static str,
    pub rollback_revision: String,
    pub checkpoint: String,
    pub materialized: bool,
    pub wal_checkpointed: bool,
    pub duration_ms: u64,
    pub checkpoint_ms: u64,
    pub locked_rollback_ms: u64,
    pub wal_checkpoint_ms: u64,
    pub materialization_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_work: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetachedBlobLocator {
    pub(crate) draft_database: PathBuf,
    pub(crate) source_id: i64,
    pub(crate) content_hash: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetachedChangesetExport {
    pub(crate) intent: DetachedChangesetIntent,
    pub(crate) blobs: Vec<DetachedBlobLocator>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct DetachedChangesetReplay {
    pub(crate) name: String,
    pub(crate) changeset_id: String,
    pub(crate) status: &'static str,
    pub(crate) created: bool,
    pub(crate) completed_items: usize,
}

pub fn begin(live: &StorePath, name: &str) -> Result<ChangesetBeginResponse> {
    let started = Instant::now();
    validate_name(name)?;
    let path = draft_path(live, name, true)?;
    reject_existing_draft(&path)?;
    remove_draft_runtime(&path)?;

    let live_store = Store::open_for_read(scope_name(live.scope), &live.path)?;
    let base = live_store.identity()?;
    let schema = live_store.schema_show()?.schema.unwrap_or_default();
    let purpose = live_store.purpose_show()?.purpose.unwrap_or_default();
    let max_source_id = live_store.max_source_id()?;

    let result = (|| -> Result<ChangesetDraftState> {
        let (mut draft, _) = Store::initialize(scope_name(live.scope), &path)?;
        let state = draft.changeset_begin_sparse(name, &base, &schema, &purpose, max_source_id)?;
        fs::create_dir(crate::scope::database_runtime_root(&path)?)?;
        Ok(state)
    })();
    let state = match result {
        Ok(state) => state,
        Err(error) => {
            let _ = remove_draft_files(&path);
            return Err(error);
        }
    };
    Ok(ChangesetBeginResponse {
        scope: scope_name(live.scope),
        database: path,
        changeset_id: state.id,
        name: state.name,
        status: state.status,
        base_revision: state.base_revision,
        duration_ms: elapsed_millis(started),
    })
}

pub fn resolve_effective(live: StorePath, name: Option<&str>) -> Result<StorePath> {
    let Some(name) = name else {
        return Ok(live);
    };
    let path = draft_path(&live, name, false)?;
    validate_draft_binding(&live, name, &path, 0)?;
    ensure_draft_runtime(&path)?;
    Ok(live.with_database(path))
}

pub fn prepare_page_touch(
    live: &StorePath,
    name: &str,
    slug: &str,
    source_ids: &[i64],
) -> Result<()> {
    let path = draft_path(live, name, false)?;
    validate_draft_binding(live, name, &path, 0)?;
    let mut draft = Store::open(scope_name(live.scope), &path)?;
    let sparse = draft.changeset_storage_kind()?.as_deref() == Some("sparse-v1");
    if sparse {
        draft.changeset_prepare_page_touch(&live.path, slug, source_ids)?;
    }
    Ok(())
}

pub fn prepare_tag_touch(
    live: &StorePath,
    name: &str,
    tag: &str,
    page: Option<&str>,
    require_member: bool,
) -> Result<()> {
    let path = draft_path(live, name, false)?;
    validate_draft_binding(live, name, &path, 0)?;
    let member = if let Some(page) = page {
        Some(page.to_string())
    } else if require_member {
        Store::open_for_read(scope_name(live.scope), &live.path)?.tag_first_page(tag)?
    } else {
        None
    };
    let mut draft = Store::open(scope_name(live.scope), &path)?;
    if draft.changeset_storage_kind()?.as_deref() == Some("sparse-v1") {
        draft.changeset_prepare_tag_touch(&live.path, tag, member.as_deref())?;
    }
    Ok(())
}

pub fn show(live: &StorePath, name: &str, limit: usize) -> Result<ChangesetShowResponse> {
    validate_name(name)?;
    let path = draft_path(live, name, false)?;
    show_path(live, name, path, limit)
}

pub fn list(live: &StorePath, limit: usize) -> Result<ChangesetListResponse> {
    let directory = changeset_directory(live, false)?;
    if !directory.exists() {
        return Ok(ChangesetListResponse {
            scope: scope_name(live.scope),
            database: live.path.clone(),
            changesets: Vec::new(),
        });
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_path(&path));
        }
        if !metadata.is_file() || path.extension().and_then(|value| value.to_str()) != Some("db") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            return Err(invalid_path(&path));
        };
        validate_name(name)?;
        names.push(name.to_string());
    }
    names.sort();
    let mut changesets = Vec::with_capacity(names.len().min(limit));
    for name in names.into_iter().take(limit) {
        changesets.push(show(live, &name, 0)?);
    }
    Ok(ChangesetListResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changesets,
    })
}

pub(crate) fn hook_summary(live: &StorePath, deadline: Instant) -> Result<ChangesetHookSummary> {
    ensure_hook_deadline(deadline)?;
    let directory = changeset_directory(live, false);
    ensure_hook_deadline(deadline)?;
    let directory = directory.map_err(|_| changeset_hook_unavailable())?;
    if !directory.exists() {
        ensure_hook_deadline(deadline)?;
        return Ok(ChangesetHookSummary {
            changesets: Vec::new(),
            omitted: 0,
        });
    }

    let entries = fs::read_dir(&directory);
    ensure_hook_deadline(deadline)?;
    let mut entries = entries.map_err(|_| changeset_hook_unavailable())?;
    let mut drafts = Vec::new();
    for _ in 0..CHANGESET_HOOK_MAX_SCAN_ITEMS {
        ensure_hook_deadline(deadline)?;
        let Some(entry) = entries.next() else { break };
        ensure_hook_deadline(deadline)?;
        let entry = entry.map_err(|_| changeset_hook_unavailable())?;
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(changeset_hook_unavailable()),
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("db")
        {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if validate_name(name).is_err() {
            continue;
        }
        drafts.push((name.to_owned(), path));
    }
    ensure_hook_deadline(deadline)?;
    let exceeds_scan_limit = entries.next().is_some();
    ensure_hook_deadline(deadline)?;
    if exceeds_scan_limit {
        return Err(AppError::new(
            "changeset_hook_limit",
            "changeset directory exceeds the fixed Hook scan limit",
        ));
    }
    drafts.sort_by(|left, right| left.0.cmp(&right.0));

    ensure_hook_deadline(deadline)?;
    let live_store = Store::open_for_hook_with_timeout(
        scope_name(live.scope),
        &live.path,
        std::time::Duration::ZERO,
    );
    ensure_hook_deadline(deadline)?;
    let live_store = live_store.map_err(|_| changeset_hook_unavailable())?;
    let snapshot = live_store.begin_hook_snapshot_with_timeout(std::time::Duration::ZERO);
    ensure_hook_deadline(deadline)?;
    snapshot.map_err(|_| changeset_hook_unavailable())?;
    let live_identity = live_store.identity();
    ensure_hook_deadline(deadline)?;
    let live_identity = live_identity.map_err(|_| changeset_hook_unavailable())?;
    ensure_hook_deadline(deadline)?;
    let mut candidates = Vec::new();
    for (name, path) in drafts {
        ensure_hook_deadline(deadline)?;
        let candidate = (|| -> Result<ChangesetHookSummaryItem> {
            require_regular_file(&path)?;
            let draft = Store::open_for_hook_with_timeout(
                scope_name(live.scope),
                &path,
                std::time::Duration::ZERO,
            )?;
            draft.begin_hook_snapshot_with_timeout(std::time::Duration::ZERO)?;
            let state = draft.changeset_draft(&name, 0)?;
            let draft_identity = draft.identity()?;
            if state.name != name || state.status != "draft" {
                return Err(AppError::new(
                    "changeset_hook_invalid",
                    "changeset draft metadata is inconsistent",
                ));
            }
            validate_id(&state.id)?;
            let sparse = draft.changeset_storage_kind()?.as_deref() == Some("sparse-v1");
            let conflict = live_identity.store_id != draft_identity.store_id
                || (!sparse && live_identity.revision != state.base_revision);
            Ok(ChangesetHookSummaryItem {
                id: state.id,
                name: state.name,
                status: state.status,
                staged_operation_count: state.staged_operation_count,
                empty: state.staged_operation_count == 0,
                conflict,
            })
        })();
        ensure_hook_deadline(deadline)?;
        match candidate {
            Ok(candidate) if candidate.conflict || !candidate.empty => {
                candidates.push(candidate);
            }
            Ok(_) => {}
            Err(_) => return Err(changeset_hook_unavailable()),
        }
    }
    ensure_hook_deadline(deadline)?;
    candidates.sort_by(|left, right| {
        right
            .conflict
            .cmp(&left.conflict)
            .then_with(|| left.name.cmp(&right.name))
    });
    let omitted = candidates.len().saturating_sub(CHANGESET_HOOK_MAX_ITEMS);
    candidates.truncate(CHANGESET_HOOK_MAX_ITEMS);
    Ok(ChangesetHookSummary {
        changesets: candidates,
        omitted,
    })
}

fn changeset_hook_unavailable() -> AppError {
    AppError::new(
        "changeset_hook_unavailable",
        "changeset Hook state could not be verified",
    )
}

fn ensure_hook_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(AppError::new(
            "changeset_hook_timeout",
            "changeset Hook summary exceeded its fixed deadline",
        ));
    }
    Ok(())
}

pub(crate) fn export_detached_intents(live: &StorePath) -> Result<Vec<DetachedChangesetExport>> {
    const MAX_DRAFTS: usize = 32;
    let directory = changeset_directory(live, false)?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut drafts = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_path(&path));
        }
        if !metadata.is_file() || path.extension().and_then(|value| value.to_str()) != Some("db") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid_path(&path))?;
        validate_name(name)?;
        drafts.push((name.to_string(), path));
    }
    drafts.sort_by(|left, right| left.0.cmp(&right.0));
    if drafts.len() > MAX_DRAFTS {
        return Err(AppError::new(
            "changeset_sync_limit",
            "too many draft changesets to export safely",
        ));
    }
    let mut exports = Vec::new();
    for (name, path) in drafts {
        validate_draft_binding(live, &name, &path, 0)?;
        let draft = Store::open_for_read(scope_name(live.scope), &path)?;
        draft.begin_read_snapshot()?;
        let _ = draft.identity()?;
        let Some((intent, blobs)) = draft.detached_changeset_intent()? else {
            continue;
        };
        exports.push(DetachedChangesetExport {
            intent,
            blobs: blobs
                .into_iter()
                .map(|(source_id, content_hash, bytes)| DetachedBlobLocator {
                    draft_database: path.clone(),
                    source_id,
                    content_hash,
                    bytes,
                })
                .collect(),
        });
    }
    Ok(exports)
}

pub(crate) fn replay_detached_intent<F>(
    live: &StorePath,
    origin_store_id: &str,
    intent: &DetachedChangesetIntent,
    mut resolve_content: F,
) -> Result<DetachedChangesetReplay>
where
    F: FnMut(&str) -> Result<PathBuf>,
{
    validate_replay_intent(live, origin_store_id, intent)?;
    let name = format!(
        "sync-{}-{}",
        &origin_store_id[..12],
        &intent.origin_changeset_id[..12]
    );
    let path = draft_path(live, &name, true)?;
    let created = !path.exists();
    if created {
        begin(live, &name)?;
    } else {
        require_regular_file(&path)?;
        validate_draft_binding(live, &name, &path, 0)?;
    }
    let mut draft = Store::open(scope_name(live.scope), &path)?;
    let state = draft.changeset_sync_replay_state(origin_store_id, &intent.origin_changeset_id)?;
    if created {
        if state.is_some() {
            return Err(replay_conflict(
                "fresh replay draft already contains replay state",
            ));
        }
        draft.changeset_sync_replay_marker(origin_store_id, &intent.origin_changeset_id, false)?;
    } else if state.is_none() {
        return Err(replay_conflict(
            "deterministic replay draft name is owned by the user",
        ));
    }
    let state = draft
        .changeset_sync_replay_state(origin_store_id, &intent.origin_changeset_id)?
        .expect("replay start marker was written");
    if state.complete {
        let changeset_id = draft.changeset_draft(&name, 0)?.id;
        return Ok(DetachedChangesetReplay {
            name,
            changeset_id,
            status: "complete",
            created: false,
            completed_items: state.items.len(),
        });
    }
    drop(draft);

    for source in intent
        .sources
        .iter()
        .filter(|source| !source.content_required)
    {
        Store::open(scope_name(live.scope), &path)?
            .changeset_replay_prepare_existing_source(&live.path, source)?;
    }

    let mut remaining_blob_bytes = 8_u64 * 1024 * 1024 * 1024;
    for source in &intent.sources {
        if !source.content_required {
            continue;
        }
        let key = format!("source\0{}", source.content_hash);
        let digest = replay_item_digest(source)?;
        if replay_item_begin(
            live,
            &name,
            origin_store_id,
            intent,
            &key,
            &digest,
            |draft| draft.changeset_replay_source_matches(source),
        )? {
            continue;
        }
        let normalized = resolve_content(&source.content_hash)?;
        let mut draft = Store::open(scope_name(live.scope), &path)?;
        let (_, copied_bytes) = draft.changeset_replay_source_from_normalized(
            &normalized,
            source,
            remaining_blob_bytes,
        )?;
        remaining_blob_bytes = remaining_blob_bytes
            .checked_sub(copied_bytes)
            .ok_or_else(|| {
                AppError::new(
                    "changeset_sync_limit",
                    "resolved source blob bytes overflowed",
                )
            })?;
        draft.changeset_sync_replay_mark_item(
            origin_store_id,
            &intent.origin_changeset_id,
            &key,
            &digest,
        )?;
    }

    for page in &intent.pages {
        let key = format!("page\0{}", page.slug);
        let digest = replay_item_digest(page)?;
        if replay_item_begin(
            live,
            &name,
            origin_store_id,
            intent,
            &key,
            &digest,
            |draft| draft.changeset_replay_page_matches(page),
        )? {
            continue;
        }
        let mut source_ids = Vec::new();
        if let Some(after) = &page.after {
            let draft = Store::open_for_read(scope_name(live.scope), &path)?;
            let live_store = Store::open_for_read(scope_name(live.scope), &live.path)?;
            for hash in &after.source_hashes {
                source_ids.push(
                    draft
                        .source_id_by_content_hash(hash)?
                        .or(live_store.source_id_by_content_hash(hash)?)
                        .ok_or_else(|| {
                            AppError::new(
                                "changeset_replay_invalid",
                                "page cites a source absent from the synchronized target",
                            )
                        })?,
                );
            }
        }
        prepare_page_touch(live, &name, &page.slug, &source_ids)?;
        let mut draft = Store::open(scope_name(live.scope), &path)?;
        if let Some(after) = &page.after {
            draft.page_put(crate::store::PagePutInput {
                slug: page.slug.clone(),
                title: after.title.clone(),
                kind: after.kind.clone(),
                summary: after.summary.clone(),
                body: after.body.clone(),
                source_ids,
                provenance: after.provenance.clone(),
            })?;
        } else {
            draft.page_remove(&page.slug)?;
        }
        draft.changeset_sync_replay_mark_item(
            origin_store_id,
            &intent.origin_changeset_id,
            &key,
            &digest,
        )?;
    }

    for meta in &intent.meta {
        let key = format!("meta\0{}", meta.key);
        let digest = replay_item_digest(meta)?;
        if replay_item_begin(
            live,
            &name,
            origin_store_id,
            intent,
            &key,
            &digest,
            |draft| draft.changeset_replay_meta_matches(meta),
        )? {
            continue;
        }
        let mut draft = Store::open(scope_name(live.scope), &path)?;
        match meta.key.as_str() {
            "schema" => {
                draft.schema_set(&meta.value)?;
            }
            "purpose" => {
                draft.purpose_set(&meta.value)?;
            }
            _ => {
                return Err(AppError::new(
                    "changeset_replay_invalid",
                    "unsupported meta key",
                ));
            }
        }
        draft.changeset_sync_replay_mark_item(
            origin_store_id,
            &intent.origin_changeset_id,
            &key,
            &digest,
        )?;
    }

    for tag in &intent.tags {
        let key = format!("tag\0{}", tag.name);
        let digest = replay_item_digest(tag)?;
        if replay_item_begin(
            live,
            &name,
            origin_store_id,
            intent,
            &key,
            &digest,
            |draft| draft.changeset_replay_tag_matches(tag),
        )? {
            continue;
        }
        if let Some(after) = &tag.after {
            for member in &after.memberships {
                prepare_tag_touch(live, &name, &tag.name, Some(&member.page_slug), false)?;
            }
        } else {
            prepare_tag_touch(live, &name, &tag.name, None, false)?;
        }
        let mut draft = Store::open(scope_name(live.scope), &path)?;
        if let Some(after) = &tag.after {
            let existed = draft.tag_exists_for_replay(&tag.name)?;
            if !existed && after.memberships.is_empty() {
                return Err(AppError::new(
                    "changeset_replay_invalid",
                    "detached intent cannot create an empty tag through typed APIs",
                ));
            }
            for member in &after.memberships {
                draft.tag_set(
                    &tag.name,
                    &member.page_slug,
                    member.priority,
                    &member.reason,
                )?;
            }
            let desired = after
                .memberships
                .iter()
                .map(|member| member.page_slug.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            for member in draft.tag_page_identities(&tag.name, 10_001)? {
                if !desired.contains(member.page_slug.as_str()) {
                    draft.tag_remove(&tag.name, &member.page_slug)?;
                }
            }
            draft.tag_autoload(
                &tag.name,
                after.autoload,
                after.autoload_priority,
                usize::try_from(after.autoload_limit).map_err(|_| {
                    AppError::new("changeset_replay_invalid", "invalid tag autoload limit")
                })?,
                usize::try_from(after.autoload_max_chars).map_err(|_| {
                    AppError::new("changeset_replay_invalid", "invalid tag autoload max chars")
                })?,
                &after.reason,
            )?;
        } else if draft.tag_exists_for_replay(&tag.name)? {
            draft.tag_delete(&tag.name)?;
        }
        draft.changeset_sync_replay_mark_item(
            origin_store_id,
            &intent.origin_changeset_id,
            &key,
            &digest,
        )?;
    }

    for source in &intent.sources {
        let key = format!("ingest\0{}", source.content_hash);
        let digest = replay_item_digest(&source.ingest)?;
        if replay_item_begin(
            live,
            &name,
            origin_store_id,
            intent,
            &key,
            &digest,
            |draft| draft.changeset_replay_ingest_matches(&source.content_hash, &source.ingest),
        )? {
            continue;
        }
        let mut draft = Store::open(scope_name(live.scope), &path)?;
        let source_id = draft
            .source_id_by_content_hash(&source.content_hash)?
            .ok_or_else(|| {
                AppError::new("changeset_replay_invalid", "replayed source is missing")
            })?;
        replay_ingest_after_image(&mut draft, source_id, &source.ingest)?;
        draft.changeset_sync_replay_mark_item(
            origin_store_id,
            &intent.origin_changeset_id,
            &key,
            &digest,
        )?;
    }

    let mut draft = Store::open(scope_name(live.scope), &path)?;
    draft.changeset_sync_replay_marker(origin_store_id, &intent.origin_changeset_id, true)?;
    let final_state = draft
        .changeset_sync_replay_state(origin_store_id, &intent.origin_changeset_id)?
        .expect("completed replay retains start marker");
    let changeset_id = draft.changeset_draft(&name, 0)?.id;
    Ok(DetachedChangesetReplay {
        name,
        changeset_id,
        status: "complete",
        created,
        completed_items: final_state.items.len(),
    })
}

fn replay_item_begin<F>(
    live: &StorePath,
    name: &str,
    origin_store_id: &str,
    intent: &DetachedChangesetIntent,
    key: &str,
    digest: &str,
    matches_after_image: F,
) -> Result<bool>
where
    F: FnOnce(&Store) -> Result<bool>,
{
    let path = draft_path(live, name, false)?;
    let mut draft = Store::open(scope_name(live.scope), path)?;
    match draft.changeset_sync_replay_start_item(
        origin_store_id,
        &intent.origin_changeset_id,
        key,
        digest,
    )? {
        ChangesetSyncReplayItemState::Complete => Ok(true),
        ChangesetSyncReplayItemState::Ready | ChangesetSyncReplayItemState::PendingClean => {
            Ok(false)
        }
        ChangesetSyncReplayItemState::PendingMutated
            if draft.changeset_sync_replay_pending_mutations_match(
                origin_store_id,
                &intent.origin_changeset_id,
                key,
            )? && matches_after_image(&draft)? =>
        {
            draft.changeset_sync_replay_mark_item(
                origin_store_id,
                &intent.origin_changeset_id,
                key,
                digest,
            )?;
            Ok(true)
        }
        ChangesetSyncReplayItemState::PendingMutated => Err(replay_conflict(
            "pending replay item differs from its declared after-image",
        )),
    }
}

fn validate_replay_intent(
    live: &StorePath,
    origin_store_id: &str,
    intent: &DetachedChangesetIntent,
) -> Result<()> {
    for value in [origin_store_id, intent.origin_changeset_id.as_str()] {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(AppError::new(
                "changeset_replay_invalid",
                "replay identity must use 64 lowercase hexadecimal characters",
            ));
        }
    }
    if intent.version != 1 {
        return Err(AppError::new(
            "changeset_replay_invalid",
            "unsupported detached intent version",
        ));
    }
    if Store::open_for_read(scope_name(live.scope), &live.path)?
        .identity()?
        .store_id
        == origin_store_id
    {
        return Err(AppError::new(
            "changeset_replay_origin_local",
            "a store cannot replay its own detached changeset intent",
        ));
    }
    let encoded = serde_json::to_vec(intent)
        .map_err(|error| AppError::new("changeset_replay_invalid", error.to_string()))?;
    if encoded.len() > 8 * 1024 * 1024
        || intent.sources.len() + intent.pages.len() + intent.tags.len() + intent.meta.len() > 1_024
        || intent.actions.len() > 2_048
    {
        return Err(AppError::new(
            "changeset_sync_limit",
            "detached replay intent exceeds fixed limits",
        ));
    }
    let mut hashes = std::collections::BTreeSet::new();
    for source in &intent.sources {
        if (source.content_required && source.base_fingerprint != "absent")
            || (!source.content_required && !replay_fingerprint_is_valid(&source.base_fingerprint))
            || source.content_hash.len() != 64
            || !source
                .content_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !hashes.insert(&source.content_hash)
            || source
                .origin
                .as_ref()
                .is_some_and(|origin| !replay_origin_is_portable(origin))
            || source.ingest.attempts < 0
            || source.ingest.attempts > 100
            || !matches!(
                source.ingest.status.as_str(),
                "pending" | "analyzing" | "generating" | "completed" | "failed"
            )
        {
            return Err(AppError::new(
                "changeset_replay_invalid",
                "malformed detached source intent",
            ));
        }
    }
    let mut pages = std::collections::BTreeSet::new();
    for page in &intent.pages {
        if !pages.insert(&page.slug) || !replay_fingerprint_is_valid(&page.base_fingerprint) {
            return Err(AppError::new(
                "changeset_replay_invalid",
                "malformed detached page intent",
            ));
        }
    }
    let mut tags = std::collections::BTreeSet::new();
    for tag in &intent.tags {
        if !tags.insert(&tag.name) || !replay_fingerprint_is_valid(&tag.base_fingerprint) {
            return Err(AppError::new(
                "changeset_replay_invalid",
                "malformed detached tag intent",
            ));
        }
    }
    let mut meta = std::collections::BTreeSet::new();
    for item in &intent.meta {
        if !matches!(item.key.as_str(), "schema" | "purpose")
            || !meta.insert(&item.key)
            || !replay_fingerprint_is_valid(&item.base_fingerprint)
        {
            return Err(AppError::new(
                "changeset_replay_invalid",
                "malformed detached meta intent",
            ));
        }
    }
    let mut action_sources = std::collections::BTreeSet::new();
    let mut action_ingests = std::collections::BTreeSet::new();
    let mut action_pages = std::collections::BTreeSet::new();
    let mut action_tags = std::collections::BTreeSet::new();
    let mut action_meta = std::collections::BTreeSet::new();
    for action in &intent.actions {
        match action {
            crate::store::DetachedChangesetAction::SourceAdd { content_hash } => {
                if !replay_fingerprint_is_valid(content_hash) {
                    return Err(AppError::new(
                        "changeset_replay_invalid",
                        "invalid source action hash",
                    ));
                }
                action_sources.insert(content_hash.as_str());
            }
            crate::store::DetachedChangesetAction::Ingest {
                action,
                content_hash,
            } => {
                if !matches!(
                    action.as_str(),
                    "ingest_claim"
                        | "ingest_analyze"
                        | "ingest_complete"
                        | "ingest_fail"
                        | "ingest_retry"
                ) || !replay_fingerprint_is_valid(content_hash)
                {
                    return Err(AppError::new(
                        "changeset_replay_invalid",
                        "invalid ingest action",
                    ));
                }
                action_ingests.insert(content_hash.as_str());
            }
            crate::store::DetachedChangesetAction::PagePut { slug } => {
                if !intent
                    .pages
                    .iter()
                    .any(|page| page.slug == *slug && page.after.is_some())
                {
                    return Err(AppError::new(
                        "changeset_replay_invalid",
                        "page_put lacks matching after-image",
                    ));
                }
                action_pages.insert(slug.as_str());
            }
            crate::store::DetachedChangesetAction::PageRemove { slug } => {
                if !intent
                    .pages
                    .iter()
                    .any(|page| page.slug == *slug && page.after.is_none())
                {
                    return Err(AppError::new(
                        "changeset_replay_invalid",
                        "page_remove lacks matching after-image",
                    ));
                }
                action_pages.insert(slug.as_str());
            }
            crate::store::DetachedChangesetAction::Tag { action, name } => {
                if !matches!(
                    action.as_str(),
                    "tag_set" | "tag_remove" | "tag_delete" | "tag_autoload"
                ) || !tags.contains(name)
                {
                    return Err(AppError::new(
                        "changeset_replay_invalid",
                        "tag action lacks matching after-image",
                    ));
                }
                action_tags.insert(name.as_str());
            }
            crate::store::DetachedChangesetAction::MetaSet { key } => {
                if !meta.contains(key) {
                    return Err(AppError::new(
                        "changeset_replay_invalid",
                        "meta action lacks matching after-image",
                    ));
                }
                action_meta.insert(key.as_str());
            }
            crate::store::DetachedChangesetAction::Search => {}
        }
    }
    if intent.sources.iter().any(|source| {
        (source.content_required && !action_sources.contains(source.content_hash.as_str()))
            || (!source.content_required && !action_ingests.contains(source.content_hash.as_str()))
    }) || pages
        .iter()
        .any(|page| !action_pages.contains(page.as_str()))
        || tags.iter().any(|tag| !action_tags.contains(tag.as_str()))
        || meta.iter().any(|key| !action_meta.contains(key.as_str()))
    {
        return Err(AppError::new(
            "changeset_replay_invalid",
            "detached after-images do not match the typed action list",
        ));
    }
    Ok(())
}

fn replay_fingerprint_is_valid(value: &str) -> bool {
    value == "absent"
        || (value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

fn replay_origin_is_portable(origin: &str) -> bool {
    let bytes = origin.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    !Path::new(origin).is_absolute() && !origin.starts_with(['/', '\\']) && !windows_absolute
}

fn replay_item_digest<T: Serialize>(item: &T) -> Result<String> {
    let encoded = serde_json::to_vec(item)
        .map_err(|error| AppError::new("changeset_replay_invalid", error.to_string()))?;
    Ok(replay_sha256(&encoded))
}

fn replay_ingest_after_image(
    draft: &mut Store,
    source_id: i64,
    ingest: &crate::store::DetachedIngestIntent,
) -> Result<()> {
    if ingest.attempts < 0 || ingest.attempts > 100 {
        return Err(AppError::new(
            "changeset_replay_invalid",
            "detached ingest attempt count is out of range",
        ));
    }
    let terminal_attempt = !matches!(ingest.status.as_str(), "pending") as i64;
    if ingest.attempts < terminal_attempt {
        return Err(AppError::new(
            "changeset_replay_invalid",
            "detached ingest status is inconsistent with its attempt count",
        ));
    }
    let retry_cycles = ingest.attempts - terminal_attempt;
    for _ in 0..retry_cycles {
        draft.ingest_claim(source_id, 1, None)?;
        draft.ingest_retry(source_id)?;
    }
    match ingest.status.as_str() {
        "pending" => {}
        "analyzing" => {
            draft.ingest_claim(source_id, 1, None)?;
        }
        "generating" | "completed" => {
            draft.ingest_claim(source_id, 1, None)?;
            draft.ingest_analyze(source_id, ingest.analysis.as_deref().unwrap_or(""))?;
            if ingest.status == "completed" {
                draft.ingest_complete(source_id, ingest.no_derived_pages_reason.as_deref())?;
            }
        }
        "failed" => {
            draft.ingest_claim(source_id, 1, None)?;
            if let Some(analysis) = ingest.analysis.as_deref() {
                draft.ingest_analyze(source_id, analysis)?;
            }
            draft.ingest_fail(source_id, "detached origin ingest failure")?;
        }
        _ => {
            return Err(AppError::new(
                "changeset_replay_invalid",
                "unsupported detached ingest status",
            ));
        }
    }
    Ok(())
}

fn replay_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn replay_conflict(message: &str) -> AppError {
    AppError::new("changeset_replay_conflict", message).with_details(json!({"mutated": false}))
}

pub fn discard(live: &StorePath, name: &str) -> Result<ChangesetDiscardResponse> {
    validate_name(name)?;
    let path = draft_path(live, name, false)?;
    let state = validate_draft_binding(live, name, &path, 0)?;
    remove_draft_files(&path)?;
    Ok(ChangesetDiscardResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changeset_id: state.id,
        name: state.name,
        status: "discarded",
    })
}

pub fn lint(
    live: &StorePath,
    name: &str,
    limit: usize,
    offset: usize,
) -> Result<crate::store::LintResponse> {
    let path = draft_path(live, name, false)?;
    validate_draft_binding(live, name, &path, 0)?;
    let draft = Store::open_for_read(scope_name(live.scope), &path)?;
    if draft.changeset_storage_kind()?.as_deref() != Some("sparse-v1") {
        return draft.lint(limit, offset);
    }
    Store::open(scope_name(live.scope), &live.path)?.changeset_sparse_lint(&path, limit, offset)
}

pub fn commit(
    live: &StorePath,
    name: &str,
    allow_lint_issues: bool,
    reason: Option<&str>,
) -> Result<ChangesetCommitResponse> {
    let started = Instant::now();
    validate_lint_override(allow_lint_issues, reason)?;
    validate_name(name)?;
    let path = draft_path(live, name, false)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(invalid_path(&path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::new(
                "changeset_not_found",
                format!("draft changeset not found: {name}"),
            ));
        }
        Err(error) => return Err(error.into()),
    }
    let state = validate_draft_binding(live, name, &path, 0)?;
    if state.staged_operation_count == 0 {
        return Err(AppError::new(
            "changeset_empty",
            "changeset has no staged operation after begin",
        ));
    }

    let live_reader = Store::open_for_read(scope_name(live.scope), &live.path)?;
    if let Some(committed) = live_reader.changeset_committed_by_id(&state.id)? {
        drop(live_reader);
        let graph_work = if committed.graph_documents.is_empty() {
            let graph = crate::external_graph::passive_status(scope_name(live.scope), &live.path)?;
            if graph["engine"] == "disabled" {
                None
            } else {
                Some(crate::work::start_graph_projection(
                    scope_name(live.scope),
                    &live.path,
                )?["work"]
                    .clone())
            }
        } else {
            Store::open(scope_name(live.scope), &live.path)?
                .schedule_graph_documents(&committed.graph_documents)?
        };
        return finish_committed(live, &path, committed, started, 0, graph_work);
    }
    let live_identity = live_reader.identity()?;
    let draft = Store::open_for_read(scope_name(live.scope), &path)?;
    if live_identity.store_id != draft.identity()?.store_id {
        return Err(AppError::new(
            "changeset_scope_mismatch",
            "changeset is not bound to this live Wiki",
        ));
    }
    let sparse = draft.changeset_storage_kind()?.as_deref() == Some("sparse-v1");
    if sparse {
        for (slug, expected) in draft.changeset_touched_pages()? {
            let observed = live_reader
                .page_mutation_fingerprint(&slug)?
                .unwrap_or_else(|| "absent".into());
            if observed != expected {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("page {slug} changed after it was first touched"),
                )
                .with_details(json!({"entity_type": "page", "identifier": slug})));
            }
        }
        for (key, expected) in draft.changeset_touched_meta()? {
            if live_reader.meta_fingerprint(&key)? != expected {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("{key} changed after the changeset began"),
                )
                .with_details(json!({"entity_type": "meta", "identifier": key})));
            }
        }
        for (tag, expected) in draft.changeset_touched_tags()? {
            if live_reader.tag_fingerprint(&tag)? != expected {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("tag {tag} changed after it was first touched"),
                )
                .with_details(json!({"entity_type": "tag", "identifier": tag})));
            }
        }
    }
    draft.validate_changeset_integrity()?;
    let lint_issues = if sparse {
        Store::open(scope_name(live.scope), &live.path)?
            .changeset_sparse_lint(&path, 1, 0)?
            .blocking_total
    } else {
        draft.lint(1, 0)?.blocking_total
    };
    if lint_issues > 0 && !allow_lint_issues {
        return Err(AppError::new(
            "changeset_lint_failed",
            format!("changeset has {lint_issues} blocking lint error(s); repair it before commit"),
        ));
    }
    let mut graph_documents = draft.changeset_graph_documents()?;
    for path in draft.changeset_touched_source_paths()? {
        if let Some(source_id) = live_reader.source_path_head(&path)? {
            graph_documents.push(("source".to_string(), source_id.to_string()));
        }
    }
    drop(draft);
    let mut draft_store = Store::open(scope_name(live.scope), &path)?;
    draft_store.changeset_freeze(
        &state.id,
        &state.draft_revision,
        state.draft_operation_id,
        state.staged_operation_count,
    )?;
    drop(draft_store);
    let checkpoint_started = Instant::now();
    let checkpoint = if sparse {
        live_reader.changeset_sparse_checkpoint_create(&state.id, &path)?
    } else {
        live_reader.changeset_checkpoint_create(&state.id)?
    };
    let checkpoint_ms = elapsed_millis(checkpoint_started);
    drop(live_reader);

    let mut live_store = Store::open(scope_name(live.scope), &live.path)?;
    let committed = live_store.changeset_publish(
        &path,
        &ChangesetPublishInput {
            id: state.id,
            name: state.name,
            store_id: live_identity.store_id,
            base_revision: state.base_revision,
            draft_revision: state.draft_revision,
            draft_operation_id: state.draft_operation_id,
            staged_operation_count: state.staged_operation_count,
            checkpoint: checkpoint.checkpoint,
            lint_issues,
            lint_override_reason: reason.map(str::to_string),
            graph_documents: graph_documents.clone(),
        },
    )?;
    let graph_work = live_store
        .schedule_graph_documents(&committed.graph_documents)
        .map_err(|error| {
            AppError::new(
                "graph_projection_failed",
                "changeset committed canonically but graph Work could not be queued",
            )
            .with_details(json!({
                "canonical_committed": true,
                "changeset_id": committed.changeset_id,
                "checkpoint": committed.checkpoint,
                "cause": error.code,
                "recovery_command": format!("lwc changeset commit {}", committed.name),
            }))
        })?;
    drop(live_store);
    finish_committed(live, &path, committed, started, checkpoint_ms, graph_work)
}

fn finish_committed(
    live: &StorePath,
    path: &Path,
    committed: crate::store::ChangesetCommitState,
    started: Instant,
    checkpoint_ms: u64,
    graph_work: Option<Value>,
) -> Result<ChangesetCommitResponse> {
    let cleanup_started = Instant::now();
    if let Err(error) = remove_draft_files(path) {
        return Err(AppError::new(
            "changeset_committed_cleanup_failed",
            format!("changeset committed but draft cleanup failed: {error}"),
        )
        .with_details(json!({
            "committed": true,
            "changeset_id": committed.changeset_id,
            "checkpoint": committed.checkpoint,
            "recovery_command": format!("lwc changeset commit {}", committed.name),
        })));
    }
    let cleanup_ms = elapsed_millis(cleanup_started);
    let materialization_started = Instant::now();
    let materialized = Store::open(scope_name(live.scope), &live.path)
        .and_then(|store| store.materialize_incremental(true).map(|_| ()));
    if let Err(error) = materialized {
        return Err(AppError::new(
            "changeset_committed_materialization_failed",
            format!("changeset committed but Markdown materialization failed: {error}"),
        )
        .with_details(json!({
            "committed": true,
            "changeset_id": committed.changeset_id,
            "checkpoint": committed.checkpoint,
            "recovery_command": "lwc maintenance materialize",
        })));
    }
    let materialization_ms = elapsed_millis(materialization_started);
    let wal_checkpoint_started = Instant::now();
    let wal_checkpointed = Store::open(scope_name(live.scope), &live.path)
        .is_ok_and(|store| store.try_checkpoint_wal());
    let wal_checkpoint_ms = elapsed_millis(wal_checkpoint_started);
    Ok(ChangesetCommitResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changeset_id: committed.changeset_id,
        name: committed.name,
        status: "committed",
        base_revision: committed.base_revision,
        post_revision: committed.post_revision,
        checkpoint: committed.checkpoint,
        staged_operation_count: committed.staged_operation_count,
        lint_issues: committed.lint_issues,
        materialized: true,
        wal_checkpointed,
        duration_ms: elapsed_millis(started),
        checkpoint_ms,
        locked_publish_ms: committed.locked_publish_ms,
        wal_checkpoint_ms,
        cleanup_ms,
        materialization_ms,
        graph_work,
    })
}

pub fn rollback(live: &StorePath, changeset_id: &str) -> Result<ChangesetRollbackResponse> {
    let started = Instant::now();
    validate_id(changeset_id)?;
    let live_reader = Store::open_for_read(scope_name(live.scope), &live.path)?;
    let history = live_reader
        .changeset_history_by_id(changeset_id)?
        .ok_or_else(|| {
            AppError::new(
                "changeset_not_found",
                format!("committed changeset not found: {changeset_id}"),
            )
        })?;
    if history.status == "rolled_back" {
        let state = live_reader
            .changeset_rollback_state_by_id(changeset_id)?
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "rolled-back changeset has no rollback operation",
                )
            })?;
        drop(live_reader);
        return finish_rolled_back(live, state, started, 0);
    }
    if history.status != "committed" {
        return Err(AppError::new(
            "changeset_not_found",
            format!("changeset is not committed: {changeset_id}"),
        ));
    }
    let identity = live_reader.identity()?;
    let sparse =
        live_reader.changeset_rollback_checkpoint_validate(&history, &identity.store_id)?;
    if !sparse && history.post_revision.as_deref() != Some(identity.revision.as_str()) {
        return Err(AppError::new(
            "changeset_rollback_conflict",
            "live Wiki changed after this changeset committed",
        ));
    }
    let checkpoint_started = Instant::now();
    let checkpoint = if sparse {
        live_reader.changeset_sparse_rollback_checkpoint_create(&history)?
    } else {
        live_reader.changeset_rollback_checkpoint_create(changeset_id)?
    };
    let checkpoint_ms = elapsed_millis(checkpoint_started);
    drop(live_reader);

    let mut live_store = Store::open(scope_name(live.scope), &live.path)?;
    let rolled_back = live_store.changeset_rollback(&ChangesetRollbackInput {
        history,
        store_id: identity.store_id,
        pre_rollback_checkpoint: checkpoint.checkpoint,
    })?;
    drop(live_store);
    finish_rolled_back(live, rolled_back, started, checkpoint_ms)
}

fn finish_rolled_back(
    live: &StorePath,
    rolled_back: crate::store::ChangesetRollbackState,
    started: Instant,
    checkpoint_ms: u64,
) -> Result<ChangesetRollbackResponse> {
    let materialization_started = Instant::now();
    let materialized = Store::open(scope_name(live.scope), &live.path)
        .and_then(|store| store.materialize_incremental(true).map(|_| ()));
    if let Err(error) = materialized {
        return Err(AppError::new(
            "changeset_rolled_back_materialization_failed",
            format!("changeset rolled back but Markdown materialization failed: {error}"),
        )
        .with_details(json!({
            "rolled_back": true,
            "changeset_id": rolled_back.changeset_id,
            "checkpoint": rolled_back.checkpoint,
            "recovery_command": format!("lwc changeset rollback {}", rolled_back.changeset_id),
        })));
    }
    let materialization_ms = elapsed_millis(materialization_started);
    let graph_work = Store::open(scope_name(live.scope), &live.path)?
        .schedule_graph_documents(&rolled_back.graph_documents)
        .map_err(|error| {
            AppError::new(
                "changeset_rolled_back_graph_projection_failed",
                "changeset rolled back canonically but graph Work could not be queued",
            )
            .with_details(json!({
                "rolled_back": true,
                "changeset_id": rolled_back.changeset_id,
                "checkpoint": rolled_back.checkpoint,
                "cause": error.code,
                "recovery_command": format!("lwc changeset rollback {}", rolled_back.changeset_id),
            }))
        })?;
    let wal_checkpoint_started = Instant::now();
    let wal_checkpointed = Store::open(scope_name(live.scope), &live.path)
        .is_ok_and(|store| store.try_checkpoint_wal());
    let wal_checkpoint_ms = elapsed_millis(wal_checkpoint_started);
    Ok(ChangesetRollbackResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changeset_id: rolled_back.changeset_id,
        name: rolled_back.name,
        status: "rolled_back",
        rollback_revision: rolled_back.rollback_revision,
        checkpoint: rolled_back.checkpoint,
        materialized: true,
        wal_checkpointed,
        duration_ms: elapsed_millis(started),
        checkpoint_ms,
        locked_rollback_ms: rolled_back.locked_rollback_ms,
        wal_checkpoint_ms,
        materialization_ms,
        graph_work,
    })
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub fn reject_selector(selector: Option<&str>, command: &str) -> Result<()> {
    if selector.is_some() {
        return Err(AppError::new(
            "changeset_command_unsupported",
            format!("{command} cannot be combined with --changeset"),
        ));
    }
    Ok(())
}

fn show_path(
    live: &StorePath,
    name: &str,
    path: PathBuf,
    limit: usize,
) -> Result<ChangesetShowResponse> {
    let state = validate_draft_binding(live, name, &path, limit)?;
    let live_identity = Store::open_for_read(scope_name(live.scope), &live.path)?.identity()?;
    let draft = Store::open_for_read(scope_name(live.scope), &path)?;
    let empty = state.staged_operation_count == 0;
    let sparse = draft
        .changeset_storage_kind()?
        .as_deref()
        .is_some_and(|value| value == "sparse-v1");
    let conflict = live_identity.store_id != draft.identity()?.store_id
        || (!sparse && live_identity.revision != state.base_revision);
    Ok(ChangesetShowResponse {
        scope: scope_name(live.scope),
        database: path,
        changeset_id: state.id,
        name: state.name,
        status: state.status,
        base_revision: state.base_revision,
        draft_revision: state.draft_revision,
        staged_operation_count: state.staged_operation_count,
        action_counts: state.action_counts,
        operations: state.operations,
        empty,
        conflict,
        created_at: state.created_at,
    })
}

fn validate_draft_binding(
    live: &StorePath,
    name: &str,
    path: &Path,
    limit: usize,
) -> Result<ChangesetDraftState> {
    require_regular_file(path)?;
    let draft = Store::open_for_read(scope_name(live.scope), path)
        .map_err(|error| map_draft_error(error, name))?;
    let state = draft
        .changeset_draft(name, limit)
        .map_err(|error| map_draft_error(error, name))?;
    let live_identity = Store::open_for_read(scope_name(live.scope), &live.path)?.identity()?;
    let draft_identity = draft.identity()?;
    if live_identity.store_id != draft_identity.store_id {
        return Err(AppError::new(
            "changeset_scope_mismatch",
            format!("draft changeset {name} is not bound to the selected Wiki"),
        ));
    }
    Ok(state)
}

fn map_draft_error(error: AppError, name: &str) -> AppError {
    if error.code == "store_not_found" || error.code == "changeset_not_found" {
        AppError::new(
            "changeset_not_found",
            format!("draft changeset not found: {name}"),
        )
    } else {
        error
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || name != name.trim()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "changeset_name_invalid",
            "changeset name must be one safe filename segment of at most 80 bytes",
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(AppError::new(
        "changeset_not_found",
        "changeset id must be a 64-character hexadecimal value",
    ))
}

fn validate_lint_override(allow: bool, reason: Option<&str>) -> Result<()> {
    match (allow, reason) {
        (false, None) => return Ok(()),
        (true, Some(value)) if !value.trim().is_empty() => return Ok(()),
        _ => {}
    }
    Err(AppError::new(
        "changeset_lint_override_invalid",
        "--allow-lint-issues and a nonblank --reason must be provided together",
    ))
}

fn draft_path(live: &StorePath, name: &str, create_directory: bool) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(changeset_directory(live, create_directory)?.join(format!("{name}.db")))
}

fn changeset_directory(live: &StorePath, create: bool) -> Result<PathBuf> {
    let parent = live
        .path
        .parent()
        .ok_or_else(|| AppError::new("invalid_store_path", "database has no parent"))?;
    let directory = parent.join("changesets");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(invalid_path(&directory));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(&directory)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(directory)
}

fn reject_existing_draft(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(invalid_path(path))
        }
        Ok(_) => Err(AppError::new(
            "changeset_exists",
            format!("draft changeset already exists: {}", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn require_regular_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(invalid_path(path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(AppError::new(
            "changeset_not_found",
            format!("draft changeset not found: {}", path.display()),
        ))?,
        Err(error) => return Err(error.into()),
    }
    for sidecar in [
        database_sidecar(path, "-wal"),
        database_sidecar(path, "-shm"),
    ] {
        match fs::symlink_metadata(&sidecar) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_path(&sidecar));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn remove_draft_files(database: &Path) -> Result<()> {
    let paths = [
        database_sidecar(database, "-wal"),
        database_sidecar(database, "-shm"),
        database.to_path_buf(),
    ];
    for path in &paths {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_path(path));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    remove_draft_runtime(database)?;
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn remove_draft_runtime(database: &Path) -> Result<()> {
    let runtime = crate::scope::database_runtime_root(database)?;
    match fs::symlink_metadata(&runtime) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(invalid_path(&runtime))
        }
        Ok(_) => fs::remove_dir_all(&runtime).map_err(Into::into),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_draft_runtime(database: &Path) -> Result<()> {
    let runtime = crate::scope::database_runtime_root(database)?;
    match fs::symlink_metadata(&runtime) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(invalid_path(&runtime))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&runtime)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn database_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}

fn invalid_path(path: &Path) -> AppError {
    AppError::new(
        "changeset_path_invalid",
        format!(
            "changeset path is not a regular owned path: {}",
            path.display()
        ),
    )
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}
