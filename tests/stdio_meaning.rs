use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PASTA: &str = "Boil salted water, add dried pasta, and simmer until tender.";
const NOTES: &[&str] = &[
    PASTA,
    "File the tax forms before the April deadline.",
    "A kubernetes pod restarts when its container exits.",
    "Practice piano scales with both hands every morning.",
    "Volcano ash grounded flights across the island.",
    "The chess opening developed the queen early.",
    "Bicycle gears make the climb easier.",
    "Roman concrete hardened under seawater.",
    "Feed the sourdough starter with flour and water.",
    "Photosynthesis stores energy from sunlight in leaves.",
];

struct Session {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    incoming: Receiver<std::io::Result<String>>,
    reader: Option<JoinHandle<()>>,
    stderr_path: PathBuf,
    next_id: i64,
}

impl Session {
    fn start(data: &Path, stderr_path: &Path) -> Self {
        let stderr = std::fs::File::create(stderr_path).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_mneme"))
            .arg("--data")
            .arg(data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("mneme binary starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, incoming) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut lines = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match lines.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = sender.send(Err(err));
                        break;
                    }
                }
            }
        });
        Self {
            child: Some(child),
            stdin: Some(stdin),
            incoming,
            reader: Some(reader),
            stderr_path: stderr_path.to_path_buf(),
            next_id: 1,
        }
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }

    fn send(&mut self, value: &serde_json::Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", serde_json::to_string(value).unwrap()).unwrap();
        stdin.flush().unwrap();
    }

    fn recv(&self, timeout: Duration) -> String {
        match self.incoming.recv_timeout(timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(err)) => panic!("stdout read failed: {err}\n{}", self.stderr()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("timed out waiting for the server\n{}", self.stderr())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("server stdout closed\n{}", self.stderr())
            }
        }
    }

    fn response(&self, id: i64, timeout: Duration) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for response {id}\n{}", self.stderr());
            }
            let line = self.recv(remaining);
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("Content-Length") {
                panic!(
                    "server wrote Content-Length framing: {trimmed}\n{}",
                    self.stderr()
                );
            }
            let value: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_else(|err| {
                panic!("stdout is not json: {trimmed}\n{err}\n{}", self.stderr())
            });
            if value.get("id").and_then(|item| item.as_i64()) == Some(id) {
                if let Some(err) = value.get("error") {
                    panic!("rpc {id} failed: {err}\n{}", self.stderr());
                }
                return value
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
            }
        }
    }

    fn initialize(&mut self, timeout: Duration) {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "mneme-test", "version": "0.0.0"}
            }
        }));
        let _ = self.response(id, timeout);
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn call(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }));
        self.response(id, timeout)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.stdin.take());
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn tool_payload(result: &serde_json::Value) -> serde_json::Value {
    if let Some(structured) = result.get("structuredContent") {
        return structured.clone();
    }
    let text = result
        .pointer("/content/0/text")
        .and_then(|item| item.as_str())
        .unwrap_or_else(|| panic!("tool result has no json: {result}"));
    serde_json::from_str(text).unwrap_or_else(|err| panic!("tool text is not json: {text} ({err})"))
}

fn assert_pasta_first(payload: &serde_json::Value, id: &str) {
    let hits = payload["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("no hits: {payload}"));
    assert!(
        hits.len() >= 2,
        "need two hits to compare scores: {payload}"
    );
    assert_eq!(hits[0]["id"].as_str(), Some(id), "top hit id: {payload}");
    assert_eq!(
        hits[0]["text"].as_str(),
        Some(PASTA),
        "top hit text: {payload}"
    );
    let first = hits[0]["score"].as_f64().expect("score");
    let second = hits[1]["score"].as_f64().expect("score");
    assert!(
        first > second,
        "top score {first} is not above {second}: {payload}"
    );
}

fn scratch() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("mneme-stdio-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
#[ignore]
fn meaning_search_persists_and_isolates() {
    let root = scratch();
    eprintln!("mneme stdio scratch: {}", root.display());
    let personal = root.join("personal");
    let company = root.join("company");
    std::fs::create_dir_all(&personal).unwrap();
    std::fs::create_dir_all(&company).unwrap();
    let long = Duration::from_secs(10 * 60);
    let short = Duration::from_secs(3 * 60);

    let pasta_id = {
        let mut session = Session::start(&personal, &root.join("stderr-1.txt"));
        session.initialize(long);
        let mut pasta_id = None;
        for (index, text) in NOTES.iter().enumerate() {
            let mut arguments = serde_json::json!({ "text": text });
            if index == 0 {
                arguments["metadata"] = serde_json::json!({"topic": "food"});
            }
            let result = session.call("memory_add", arguments, short);
            let payload = tool_payload(&result);
            let id = payload["id"]
                .as_str()
                .unwrap_or_else(|| panic!("add returned no id: {payload}"))
                .to_owned();
            if index == 0 {
                pasta_id = Some(id);
            }
        }
        let pasta_id = pasta_id.expect("pasta id");
        let result = session.call(
            "memory_search",
            serde_json::json!({"query": "how do I cook noodles", "limit": 5}),
            short,
        );
        assert_pasta_first(&tool_payload(&result), &pasta_id);
        pasta_id
    };

    {
        let mut session = Session::start(&personal, &root.join("stderr-2.txt"));
        session.initialize(short);
        let result = session.call(
            "memory_search",
            serde_json::json!({"query": "how do I cook noodles"}),
            short,
        );
        assert_pasta_first(&tool_payload(&result), &pasta_id);
    }

    {
        let mut session = Session::start(&company, &root.join("stderr-3.txt"));
        session.initialize(short);
        let result = session.call(
            "memory_search",
            serde_json::json!({"query": "how do I cook noodles"}),
            short,
        );
        let payload = tool_payload(&result);
        let hits = payload["hits"]
            .as_array()
            .unwrap_or_else(|| panic!("no hits: {payload}"));
        for hit in hits {
            assert_ne!(
                hit["text"].as_str(),
                Some(PASTA),
                "a second data directory returned the personal note: {payload}"
            );
        }
    }

    std::fs::remove_dir_all(&root).unwrap();
}
