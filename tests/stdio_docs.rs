use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PASTA: &str = "Boil salted water and simmer dried pasta until tender.";

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
            .arg("run")
            .arg("--data")
            .arg(data)
            .arg("--model")
            .arg("fake")
            .arg("--dim")
            .arg("4")
            .env("MNEME_EMBEDDER", "fake")
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

    fn response(&self, id: i64, timeout: Duration) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for response {id}\n{}", self.stderr());
            }
            let line = match self.incoming.recv_timeout(remaining) {
                Ok(Ok(line)) => line,
                Ok(Err(err)) => panic!("stdout read failed: {err}\n{}", self.stderr()),
                Err(_) => panic!("timed out waiting for the server\n{}", self.stderr()),
            };
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

    fn initialize(&mut self) {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "mneme-test", "version": "0.0.0"}
            }
        }));
        let result = self.response(1, Duration::from_secs(30));
        assert_eq!(
            result["protocolVersion"].as_str(),
            Some("2025-06-18"),
            "server negotiates 2025-06-18: {result}"
        );
        self.next_id = 2;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
    }

    fn call(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }));
        self.response(id, Duration::from_secs(30))
    }

    fn read_doc(&mut self, id: &str) -> serde_json::Value {
        let rpc = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": rpc,
            "method": "resources/read",
            "params": {"uri": format!("mneme://doc/{id}")}
        }));
        self.response(rpc, Duration::from_secs(30))
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

#[test]
fn stdio_ingest_and_read_round_trip() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("mneme-stdio-docs-{stamp}"));
    std::fs::create_dir_all(&root).unwrap();
    let data = root.join("personal");
    let mut session = Session::start(&data, &root.join("stderr.txt"));
    session.initialize();
    let added = tool_payload(&session.call(
        "ingest",
        serde_json::json!({"text": PASTA, "title": "Pasta"}),
    ));
    let doc_id = added["doc_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no doc_id: {added}"));
    let read = session.read_doc(doc_id);
    let body = read
        .pointer("/contents/0/text")
        .and_then(|item| item.as_str())
        .unwrap_or_else(|| panic!("resource has no text: {read}"));
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        parsed["text"].as_str(),
        Some(PASTA),
        "resource returns the full document"
    );
    assert_eq!(parsed["kind"].as_str(), Some("doc"));
    drop(session);
    let _ = std::fs::remove_dir_all(&root);
}
