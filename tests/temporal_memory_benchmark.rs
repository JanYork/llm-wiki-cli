use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Instant,
};
use tempfile::TempDir;

struct TestWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
    binary: PathBuf,
}

impl TestWorld {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            _temp: temp,
            project,
            home,
            binary: env::var_os("LWC_BENCH_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_lwc"))),
        }
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(&self.binary)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn measured(&self, args: &[&str]) -> (Value, f64) {
        let started = Instant::now();
        let value = self.ok(args);
        (value, started.elapsed().as_secs_f64() * 1000.0)
    }

    fn remember(&self, capsule: &Value) -> Value {
        let raw = serde_json::to_string(capsule).unwrap();
        self.ok(&["remember", "--json", &raw])
    }
}

struct QueryCase {
    category: &'static str,
    query: String,
    expected_id: String,
    excluded_id: Option<String>,
    since: Option<&'static str>,
    until: Option<&'static str>,
}

#[test]
#[ignore = "focused temporal-memory quality and latency benchmark"]
fn temporal_memory_benchmark_reports_json() {
    let query_world = TestWorld::new();
    let initialized = query_world.ok(&["init"]);
    let database = PathBuf::from(initialized["database"].as_str().unwrap());
    query_world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
        "--memory-max-bytes",
        "268435456",
    ]);

    let mut cases = Vec::new();
    let mut control_current_hits = 0usize;
    let mut control_stale_hits = 0usize;
    for index in 0..10 {
        let token = format!("legacytoken{index:02}");
        let old = query_world.remember(&json!({
            "type": "历史故障",
            "context": format!("旧状态 {token}"),
            "occurred_at": "2026-01-01T00:00:00Z",
            "observed": [format!("旧诊断只包含 {token}")]
        }));
        let old_id = old["event"]["id"].as_str().unwrap().to_owned();
        let replacement = query_world.remember(&json!({
            "type": "当前结论",
            "context": format!("已解决故障 {index:02}"),
            "occurred_at": "2026-08-01T00:00:00Z",
            "outcome": [format!("采用当前修复方案 {index:02}")],
            "relations": [{
                "type": "supersedes",
                "target": old_id,
                "basis": "验证后的显式替代"
            }]
        }));
        let expected_id = replacement["event"]["id"].as_str().unwrap().to_owned();
        let lexical_top = lexical_top_id(&database, &token);
        control_current_hits += usize::from(lexical_top.as_deref() == Some(expected_id.as_str()));
        control_stale_hits += usize::from(lexical_top.as_deref() == Some(old_id.as_str()));
        cases.push(QueryCase {
            category: "supersession",
            query: token,
            expected_id,
            excluded_id: Some(old_id),
            since: None,
            until: None,
        });
    }

    for index in 0..5 {
        let token = format!("freshsignal{index:02}");
        let event = query_world.remember(&json!({
            "type": "新鲜事实",
            "context": format!("当前事实 {token}"),
            "observed": [format!("当前唯一信号 {token}")]
        }));
        cases.push(QueryCase {
            category: "fresh",
            query: token,
            expected_id: event["event"]["id"].as_str().unwrap().to_owned(),
            excluded_id: None,
            since: None,
            until: None,
        });
    }

    for index in 0..5 {
        let token = format!("windowtoken{index:02}");
        let old = query_world.remember(&json!({
            "type": "窗口事实",
            "context": format!("旧窗口 {token}"),
            "occurred_at": "2025-01-01T00:00:00Z",
            "observed": [format!("旧窗口信号 {token}")]
        }));
        let recent = query_world.remember(&json!({
            "type": "窗口事实",
            "context": format!("新窗口 {token}"),
            "occurred_at": "2026-08-01T00:00:00Z",
            "observed": [format!("新窗口信号 {token}")]
        }));
        let old_id = old["event"]["id"].as_str().unwrap().to_owned();
        let recent_id = recent["event"]["id"].as_str().unwrap().to_owned();
        let use_since = index % 2 == 0;
        cases.push(QueryCase {
            category: "bounds",
            query: token,
            expected_id: if use_since {
                recent_id.clone()
            } else {
                old_id.clone()
            },
            excluded_id: Some(if use_since { old_id } else { recent_id }),
            since: use_since.then_some("2026-01-01"),
            until: (!use_since).then_some("2025-12-31"),
        });
    }
    assert_eq!(cases.len(), 20);

    let mut top1_hits = 0usize;
    let mut relevant = 0usize;
    let mut returned = 0usize;
    let mut stale_suppressed = 0usize;
    let mut supersession_top1_hits = 0usize;
    let mut direct_fresh_top1_hits = 0usize;
    let mut bounds_top1_hits = 0usize;
    let mut bounds_excluded = 0usize;
    let mut recall_ms = Vec::new();
    for case in &cases {
        let mut args = vec!["memory", "recall", case.query.as_str(), "--limit", "5"];
        if let Some(since) = case.since {
            args.extend(["--since", since]);
        }
        if let Some(until) = case.until {
            args.extend(["--until", until]);
        }
        let (response, elapsed_ms) = query_world.measured(&args);
        recall_ms.push(elapsed_ms);
        let results = response["results"].as_array().unwrap();
        assert!(results.len() <= 5, "recall exceeded its requested bound");
        let ids = results
            .iter()
            .map(|result| {
                result["event"]["id"]
                    .as_str()
                    .expect("every recall result must contain an event ID")
            })
            .collect::<Vec<_>>();
        let top1 = ids.first().copied() == Some(case.expected_id.as_str());
        top1_hits += usize::from(top1);
        relevant += ids
            .iter()
            .filter(|id| **id == case.expected_id.as_str())
            .count();
        returned += ids.len();
        let excluded = case
            .excluded_id
            .as_deref()
            .is_none_or(|excluded_id| !ids.contains(&excluded_id));
        match case.category {
            "supersession" => {
                supersession_top1_hits += usize::from(top1);
                stale_suppressed += usize::from(top1 && excluded);
            }
            "fresh" => direct_fresh_top1_hits += usize::from(top1),
            "bounds" => {
                bounds_top1_hits += usize::from(top1);
                bounds_excluded += usize::from(top1 && excluded);
            }
            _ => unreachable!(),
        }
    }

    let false_merge_rate = false_merge_rate();
    let (protected_survival, expired_control_eviction) = retention_quality();
    let (hint_precision, hint_recall, hint_true_positive, hint_false_positive, emitted_hint_types) =
        hint_quality();
    let (
        control_record_ms,
        feature_record_ms,
        paired_p95_ratios,
        median_p95_ratio,
        latency_age_evictions,
        latency_capacity_evictions,
    ) = record_latency();
    let fresh_top1 = rate(top1_hits, cases.len());
    let supersession_top1 = rate(supersession_top1_hits, 10);
    let direct_fresh_top1 = rate(direct_fresh_top1_hits, 5);
    let bounds_top1 = rate(bounds_top1_hits, 5);
    let bounds_exclusion = rate(bounds_excluded, 5);
    let bounded_precision = rate(relevant, returned);
    let stale_suppression = rate(stale_suppressed, 10);
    let report = json!({
        "fixture": {"labeled_queries": cases.len(), "supersession_queries": 10},
        "counts": {
            "top1_hits": top1_hits,
            "returned_results": returned,
            "relevant_results": relevant,
            "supersession_top1_hits": supersession_top1_hits,
            "stale_suppressed_with_current_hit": stale_suppressed,
            "bounds_top1_hits": bounds_top1_hits,
            "bounds_excluded_with_expected_hit": bounds_excluded,
            "hint_true_positive": hint_true_positive,
            "hint_false_positive": hint_false_positive,
            "latency_feature_age_evictions": latency_age_evictions,
            "latency_feature_capacity_evictions": latency_capacity_evictions,
        },
        "quality": {
            "control_lexical_current_top1": rate(control_current_hits, 10),
            "control_lexical_stale_top1": rate(control_stale_hits, 10),
            "fresh_top1": fresh_top1,
            "supersession_top1": supersession_top1,
            "direct_fresh_top1": direct_fresh_top1,
            "bounds_top1": bounds_top1,
            "bounds_exclusion": bounds_exclusion,
            "stale_suppression": stale_suppression,
            "bounded_precision": bounded_precision,
            "false_merge_rate": false_merge_rate,
            "protected_survival": protected_survival,
            "expired_control_eviction": expired_control_eviction,
            "hint_precision": hint_precision,
            "hint_recall": hint_recall,
            "emitted_hint_types": emitted_hint_types,
        },
        "latency_ms": {
            "recall": {"p50": percentile(&recall_ms, 50), "p95": percentile(&recall_ms, 95)},
            "record_control": {"p50": percentile(&control_record_ms, 50), "p95": percentile(&control_record_ms, 95)},
            "record_feature": {"p50": percentile(&feature_record_ms, 50), "p95": percentile(&feature_record_ms, 95)},
            "paired_run_p95_ratios": paired_p95_ratios.iter().map(|value| round3(*value)).collect::<Vec<_>>(),
            "median_run_p95_ratio": round3(median_p95_ratio),
            "runs": 5,
            "measurement": "public_cli_end_to_end",
            "control": "same fixture with inactive age and capacity thresholds",
        },
        "gates": {
            "fresh_top1_min": 0.90,
            "supersession_top1": 1.0,
            "direct_fresh_top1": 1.0,
            "bounds_top1": 1.0,
            "bounds_exclusion": 1.0,
            "stale_suppression": 1.0,
            "bounded_precision": 1.0,
            "false_merge_rate": 0.0,
            "protected_survival": 1.0,
            "expired_control_eviction": 1.0,
            "hint_precision": 1.0,
            "hint_recall_min": 0.95,
            "record_p95_ratio_max": 1.25,
            "latency_feature_age_evictions_min": 1,
            "latency_feature_capacity_evictions_min": 1,
        }
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());

    assert!(fresh_top1 >= 0.90, "fresh top-1 gate failed: {report}");
    assert_eq!(
        control_stale_hits, 10,
        "lexical baseline is invalid: {report}"
    );
    assert!(
        supersession_top1 > rate(control_current_hits, 10),
        "temporal recall did not improve over lexical current-state recall: {report}"
    );
    assert_eq!(
        supersession_top1, 1.0,
        "supersession top-1 failed: {report}"
    );
    assert_eq!(
        direct_fresh_top1, 1.0,
        "direct fresh top-1 failed: {report}"
    );
    assert_eq!(bounds_top1, 1.0, "time-bound top-1 failed: {report}");
    assert_eq!(
        bounds_exclusion, 1.0,
        "time-bound exclusion failed: {report}"
    );
    assert_eq!(stale_suppression, 1.0, "stale suppression failed: {report}");
    assert_eq!(bounded_precision, 1.0, "bounded precision failed: {report}");
    assert_eq!(false_merge_rate, 0.0, "false merge gate failed: {report}");
    assert_eq!(
        protected_survival, 1.0,
        "protected survival failed: {report}"
    );
    assert_eq!(
        expired_control_eviction, 1.0,
        "expired ordinary control was not evicted: {report}"
    );
    assert_eq!(hint_precision, 1.0, "hint precision failed: {report}");
    assert!(hint_recall >= 0.95, "hint recall failed: {report}");
    assert!(
        median_p95_ratio <= 1.25,
        "record P95 ratio gate failed: {report}"
    );
    assert!(
        latency_age_evictions > 0 && latency_capacity_evictions > 0,
        "record latency fixture did not exercise retention: {report}"
    );
}

fn lexical_top_id(database: &Path, query: &str) -> Option<String> {
    Connection::open(database)
        .unwrap()
        .query_row(
            "SELECT event_id FROM memory_fts
             WHERE memory_fts MATCH ?1 ORDER BY bm25(memory_fts) LIMIT 1",
            [query],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn false_merge_rate() -> f64 {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let identical = json!({
        "type": "重复观察", "context": "相同文本仍是独立事件",
        "observed": ["没有 request_id 就不能自动合并"]
    });
    let capsules = [
        identical.clone(),
        identical.clone(),
        identical,
        json!({
            "type": "重复观察", "context": "相似文本但实体甲不同",
            "observed": ["没有 request_id 就不能自动合并"]
        }),
        json!({
            "type": "重复观察", "context": "相似文本但实体乙不同",
            "observed": ["没有 request_id 就不能自动合并"]
        }),
    ];
    let ids = capsules
        .iter()
        .map(|capsule| {
            world.remember(capsule)["event"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    rate(capsules.len() - ids.len(), capsules.len() - 1)
}

fn retention_quality() -> (f64, f64) {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
        "--memory-max-bytes",
        "268435456",
    ]);
    let pinned = world.remember(&json!({
        "type": "保护样本", "context": "固定", "occurred_at": "2000-01-01T00:00:00Z",
        "pinned": true, "observed": ["保留"]
    }));
    let unresolved = world.remember(&json!({
        "type": "保护样本", "context": "未决", "occurred_at": "2000-01-02T00:00:00Z",
        "unresolved": ["仍待处理"]
    }));
    let contradicted = world.remember(&json!({
        "type": "保护样本", "context": "旧判断", "occurred_at": "2000-01-03T00:00:00Z",
        "decision": ["旧判断"]
    }));
    let contradiction = world.remember(&json!({
        "type": "保护样本", "context": "冲突", "occurred_at": "2000-01-04T00:00:00Z",
        "decision": ["相反判断"],
        "relations": [{"type": "contradicts", "target": contradicted["event"]["id"], "basis": "冲突"}]
    }));
    let ordinary = world.remember(&json!({
        "type": "淘汰样本", "context": "普通旧事件", "occurred_at": "2000-01-05T00:00:00Z",
        "observed": ["应淘汰"]
    }));
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "1",
        "--memory-max-bytes",
        "268435456",
    ]);
    world.ok(&["memory", "maintain"]);
    let ids = [pinned, unresolved, contradicted, contradiction]
        .into_iter()
        .map(|event| event["event"]["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let protected_survival = rate(
        ids.iter()
            .filter(|id| world.command(&["memory", "show", id]).status.success())
            .count(),
        ids.len(),
    );
    let ordinary_id = ordinary["event"]["id"].as_str().unwrap();
    let expired_control_eviction = if world
        .command(&["memory", "show", ordinary_id])
        .status
        .success()
    {
        0.0
    } else {
        1.0
    };
    (protected_survival, expired_control_eviction)
}

fn hint_quality() -> (f64, f64, usize, usize, Vec<String>) {
    let mut true_positive = 0usize;
    let mut false_positive = 0usize;
    let mut emitted_types = BTreeSet::new();

    let cluster = TestWorld::new();
    cluster.ok(&["init"]);
    let cluster_responses = (0..5)
        .map(|_| {
            cluster.remember(&json!({
                "type": "聚类", "context": "完全相同", "observed": ["确定性样本"]
            }))
        })
        .collect::<Vec<_>>();
    score_hint_scenario(
        "exact-context-cluster",
        &cluster_responses,
        &mut true_positive,
        &mut false_positive,
        &mut emitted_types,
    );

    let relation = TestWorld::new();
    relation.ok(&["init"]);
    let target = relation.remember(&json!({
        "type": "关系", "context": "旧事件", "observed": ["旧"]
    }));
    let relation_response = relation.remember(&json!({
        "type": "关系", "context": "新事件", "decision": ["新"],
        "relations": [{"type": "supersedes", "target": target["event"]["id"], "basis": "替代"}]
    }));
    score_hint_scenario(
        "relation-review",
        &[relation_response],
        &mut true_positive,
        &mut false_positive,
        &mut emitted_types,
    );

    let unresolved = TestWorld::new();
    unresolved.ok(&["init"]);
    let unresolved_response = unresolved.remember(&json!({
        "type": "未决", "context": "长期未决", "occurred_at": "2000-01-01T00:00:00Z",
        "unresolved": ["仍待确认"]
    }));
    score_hint_scenario(
        "aged-unresolved",
        &[unresolved_response],
        &mut true_positive,
        &mut false_positive,
        &mut emitted_types,
    );

    let pressure_capsule = json!({
        "type": "压力", "context": "容量接近阈值", "observed": ["压力样本"]
    });
    let sizing = TestWorld::new();
    sizing.ok(&["init"]);
    let logical_bytes = sizing.remember(&pressure_capsule)["event"]["logical_bytes"]
        .as_u64()
        .unwrap();
    let pressure = TestWorld::new();
    pressure.ok(&["init"]);
    let max_bytes = (logical_bytes * 100 / 85).to_string();
    pressure.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-bytes",
        &max_bytes,
    ]);
    let pressure_response = pressure.remember(&pressure_capsule);
    score_hint_scenario(
        "storage-pressure",
        &[pressure_response],
        &mut true_positive,
        &mut false_positive,
        &mut emitted_types,
    );

    let emitted = true_positive + false_positive;
    (
        rate(true_positive, emitted),
        rate(true_positive, 4),
        true_positive,
        false_positive,
        emitted_types.into_iter().collect(),
    )
}

fn score_hint_scenario(
    expected: &str,
    responses: &[Value],
    true_positive: &mut usize,
    false_positive: &mut usize,
    emitted_types: &mut BTreeSet<String>,
) {
    let mut found = false;
    for (index, response) in responses.iter().enumerate() {
        for hint in response["hints"].as_array().unwrap() {
            let hint_type = hint["type"].as_str().unwrap();
            emitted_types.insert(hint_type.to_owned());
            if index + 1 == responses.len() && hint_type == expected && !found {
                *true_positive += 1;
                found = true;
            } else {
                *false_positive += 1;
            }
        }
    }
}

fn record_latency() -> (Vec<f64>, Vec<f64>, Vec<f64>, f64, u64, u64) {
    let mut control_all = Vec::new();
    let mut feature_all = Vec::new();
    let mut control_run_p95 = Vec::new();
    let mut feature_run_p95 = Vec::new();
    let mut feature_age_evictions = 0u64;
    let mut feature_capacity_evictions = 0u64;
    for run in 0..5 {
        let (control, feature) = if run % 2 == 0 {
            (
                measured_record_batch(run, true),
                measured_record_batch(run, false),
            )
        } else {
            let feature = measured_record_batch(run, false);
            let control = measured_record_batch(run, true);
            (control, feature)
        };
        assert_eq!((control.1, control.2), (0, 0));
        assert!(
            feature.1 > 0 && feature.2 > 0,
            "feature run {run} did not exercise both retention paths"
        );
        feature_age_evictions += feature.1;
        feature_capacity_evictions += feature.2;
        control_run_p95.push(percentile_raw(&control.0, 95));
        feature_run_p95.push(percentile_raw(&feature.0, 95));
        control_all.extend(control.0);
        feature_all.extend(feature.0);
    }
    let paired_ratios = feature_run_p95
        .iter()
        .zip(&control_run_p95)
        .map(|(feature, control)| feature / control)
        .collect::<Vec<_>>();
    let ratio = median(&paired_ratios);
    (
        control_all,
        feature_all,
        paired_ratios,
        ratio,
        feature_age_evictions,
        feature_capacity_evictions,
    )
}

fn measured_record_batch(run: usize, control: bool) -> (Vec<f64>, u64, u64) {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        if control { "50000" } else { "365" },
        "--memory-max-bytes",
        if control { "4294967296" } else { "512" },
    ]);
    let latencies = (0..20)
        .map(|index| {
            let mut capsule = json!({
                "type": "延迟样本",
                "context": format!("run-{run}-{}-{index}", if control { "control" } else { "feature" }),
                "observed": ["相同大小的当前事件"]
            });
            if index < 5 {
                capsule["occurred_at"] = Value::String("2000-01-01T00:00:00Z".to_owned());
            }
            let raw = serde_json::to_string(&capsule).unwrap();
            world.measured(&["remember", "--json", &raw]).1
        })
        .collect();
    let status = world.ok(&["memory", "status"]);
    (
        latencies,
        status["counters"]["age_evictions"].as_u64().unwrap(),
        status["counters"]["capacity_evictions"].as_u64().unwrap(),
    )
}

fn rate(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        round3(numerator as f64 / denominator as f64)
    }
}

fn percentile(values: &[f64], percentile: usize) -> f64 {
    round3(percentile_raw(values, percentile))
}

fn percentile_raw(values: &[f64], percentile: usize) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index]
}

fn median(values: &[f64]) -> f64 {
    percentile_raw(values, 50)
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
