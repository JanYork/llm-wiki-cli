use serde_json::{Value, json};
use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::Path,
    process::{Child, Command, Stdio},
    time::Instant,
};

fn binary() -> std::path::PathBuf {
    env::var_os("LWC_BENCH_BINARY")
        .map(Into::into)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_lwc").into())
}

fn run(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = Command::new(binary())
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn start_view(project: &Path, home: &Path) -> (Child, String) {
    let mut child = Command::new(binary())
        .current_dir(project)
        .env("HOME", home)
        .args(["view", "--port", "0", "--no-open"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let startup: Value = serde_json::from_str(&line).unwrap();
    (child, startup["address"].as_str().unwrap().to_string())
}

fn request(address: &str, path: &str) -> Value {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn rss_bytes(child: &Child) -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &child.id().to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse::<u64>()
        .unwrap()
        * 1024
}

#[test]
#[ignore = "performance evidence; creates a synthetic 10k-document corpus"]
fn word_graph_stays_bounded_at_ten_thousand_documents() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    let corpus = temp.path().join("corpus");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&corpus).unwrap();
    let fixture_started = Instant::now();
    for index in 0..10_000 {
        fs::write(
            corpus.join(format!("document-{index:05}.md")),
            format!(
                "needle shared 库存 共享 group{} document{index}",
                index % 20
            ),
        )
        .unwrap();
    }
    let fixture_ms = fixture_started.elapsed().as_secs_f64() * 1000.0;

    run(&project, &home, &["init"]);
    let import_started = Instant::now();
    let imported = run(
        &project,
        &home,
        &[
            "source",
            "add-dir",
            "--allow-external-source",
            corpus.to_str().unwrap(),
        ],
    );
    let import_ms = import_started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(imported["created"], 10_000);

    let (mut view, address) = start_view(&project, &home);
    let cold_started = Instant::now();
    let warm = request(
        &address,
        "/api/graphs/words?query=needle&limit=25&term_limit=30&offset=0",
    );
    let cold_ms = cold_started.elapsed().as_secs_f64() * 1000.0;
    assert!(warm["nodes"].as_array().unwrap().len() <= 200);
    assert!(warm["edges"].as_array().unwrap().len() <= 500);
    assert!(warm["diagnostics"]["inspected_bytes"].as_u64().unwrap() <= 4 * 1024 * 1024);

    let baseline_rss = rss_bytes(&view);
    let mut peak_rss = baseline_rss;
    let mut latency_ms = Vec::new();
    let mut max_documents = 0;
    let mut max_spans = 0;
    let mut max_bytes = 0;
    let mut max_tokens = 0;
    let mut max_nodes = 0;
    let mut max_edges = 0;
    for sample in 0..30 {
        let query = if sample % 2 == 0 {
            "needle"
        } else {
            "%E5%BA%93%E5%AD%98"
        };
        let path = format!(
            "/api/graphs/words?query={query}&limit=25&term_limit=30&offset={}",
            sample * 25
        );
        let started = Instant::now();
        let graph = request(&address, &path);
        latency_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        peak_rss = peak_rss.max(rss_bytes(&view));
        max_documents = max_documents.max(
            graph["diagnostics"]["inspected_documents"]
                .as_u64()
                .unwrap(),
        );
        max_spans = max_spans.max(graph["diagnostics"]["inspected_spans"].as_u64().unwrap());
        max_bytes = max_bytes.max(graph["diagnostics"]["inspected_bytes"].as_u64().unwrap());
        max_tokens = max_tokens.max(graph["diagnostics"]["inspected_tokens"].as_u64().unwrap());
        max_nodes = max_nodes.max(graph["nodes"].as_array().unwrap().len());
        max_edges = max_edges.max(graph["edges"].as_array().unwrap().len());
        assert!(max_nodes <= 200);
        assert!(max_edges <= 500);
    }
    view.kill().unwrap();
    view.wait().unwrap();

    latency_ms.sort_by(f64::total_cmp);
    let p50_ms = latency_ms[14];
    let p95_ms = latency_ms[28];
    let rss_delta_bytes = peak_rss.saturating_sub(baseline_rss);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "documents": 10_000,
            "fixture_ms": (fixture_ms * 1000.0).round() / 1000.0,
            "import_ms": (import_ms * 1000.0).round() / 1000.0,
            "requests": latency_ms.len(),
            "queries": ["needle", "库存"],
            "cold_ms": (cold_ms * 1000.0).round() / 1000.0,
            "p50_ms": (p50_ms * 1000.0).round() / 1000.0,
            "p95_ms": (p95_ms * 1000.0).round() / 1000.0,
            "rss_delta_bytes": rss_delta_bytes,
            "rss_method": "ps -o rss=",
            "platform": {"os": env::consts::OS, "arch": env::consts::ARCH, "parallelism": std::thread::available_parallelism().unwrap().get()},
            "observed_maxima": {"documents": max_documents, "spans": max_spans, "bytes": max_bytes, "tokens": max_tokens, "nodes": max_nodes, "edges": max_edges},
            "limits": {"documents": 25, "spans": 100, "nodes": 200, "edges": 500, "inspected_bytes": 4 * 1024 * 1024},
        }))
        .unwrap()
    );
    assert!(p95_ms <= 500.0, "word graph p95 was {p95_ms:.3}ms");
    assert!(
        rss_delta_bytes <= 64 * 1024 * 1024,
        "word graph RSS delta was {rss_delta_bytes} bytes"
    );
}
