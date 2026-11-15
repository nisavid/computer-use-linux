use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_INCOMING_LINE_BYTES: usize = 16 * 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct McpServer {
    child: ChildGuard,
    stdin: Option<ChildStdin>,
    responses: Receiver<Result<Vec<u8>, String>>,
    stdout_reader: JoinHandle<()>,
    stderr_reader: JoinHandle<Vec<u8>>,
}

fn spawn_server() -> McpServer {
    let mut command = Command::new(env!("CARGO_BIN_EXE_computer-use-linux"));
    command.arg("mcp");
    spawn_command(&mut command)
}

fn spawn_command(command: &mut Command) -> McpServer {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn MCP test child");
    let stdin = child.stdin.take().expect("server stdin is piped");
    let stdout = child.stdout.take().expect("server stdout is piped");
    let mut stderr = child.stderr.take().expect("server stderr is piped");
    let (response_tx, response_rx) = mpsc::channel();
    let stdout_reader = thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match stdout.read_until(b'\n', &mut line) {
                Ok(0) => return,
                Ok(_) => {
                    if response_tx.send(Ok(line)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = response_tx.send(Err(error.to_string()));
                    return;
                }
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).expect("read server stderr");
        output
    });
    McpServer {
        child: ChildGuard(child),
        stdin: Some(stdin),
        responses: response_rx,
        stdout_reader,
        stderr_reader,
    }
}

fn padded_ping(id: u64, json_bytes: usize) -> Vec<u8> {
    let mut request = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#).into_bytes();
    assert!(request.len() <= json_bytes);
    request.resize(json_bytes, b' ');
    serde_json::from_slice::<Value>(&request).expect("padded request remains valid JSON");
    request
}

fn classify_message(message: Value, expected_id: Option<u64>) -> Result<Option<Value>, String> {
    if message.get("jsonrpc") != Some(&json!("2.0")) {
        return Err(format!("unexpected JSON-RPC message: {message}"));
    }
    if let Some(id) = message.get("id") {
        let is_response = message.get("method").is_none()
            && (message.get("result").is_some() ^ message.get("error").is_some());
        if is_response && expected_id.is_some_and(|expected| id == &json!(expected)) {
            return Ok(Some(message));
        }
        let kind = if is_response { "response" } else { "message" };
        return Err(format!("unexpected JSON-RPC {kind}: {message}"));
    }
    if message.get("method").is_some_and(Value::is_string)
        && message.get("result").is_none()
        && message.get("error").is_none()
    {
        return Ok(None);
    }
    Err(format!("unexpected JSON-RPC message: {message}"))
}

fn kill_and_reap(child: &mut Child) -> Result<ExitStatus, String> {
    if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
        return Ok(status);
    }
    child.kill().map_err(|error| error.to_string())?;
    child.wait().map_err(|error| error.to_string())
}

fn wait_for_exit(child: &mut Child) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let status = kill_and_reap(child)?;
            return Err(format!("server did not exit after stdin closed: {status}"));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

impl McpServer {
    fn send_line(&mut self, payload: Vec<u8>, timeout: Duration) -> Result<(), String> {
        let mut stdin = self
            .stdin
            .take()
            .ok_or_else(|| "server stdin is already closed".to_owned())?;
        let (completion_tx, completion_rx) = mpsc::channel();
        let writer = thread::spawn(move || {
            let result = (|| {
                stdin.write_all(&payload)?;
                stdin.write_all(b"\n")?;
                stdin.flush()
            })()
            .map_err(|error| error.to_string());
            let _ = completion_tx.send((stdin, result));
        });

        match completion_rx.recv_timeout(timeout) {
            Ok((stdin, result)) => {
                self.stdin = Some(stdin);
                writer
                    .join()
                    .map_err(|_| "request writer panicked".to_owned())?;
                result
            }
            Err(RecvTimeoutError::Timeout) => {
                kill_and_reap(&mut self.child.0)
                    .map_err(|error| format!("reap child after write timeout: {error}"))?;
                writer
                    .join()
                    .map_err(|_| "request writer panicked after timeout".to_owned())?;
                Err(format!("writing request timed out after {timeout:?}"))
            }
            Err(RecvTimeoutError::Disconnected) => {
                writer
                    .join()
                    .map_err(|_| "request writer panicked".to_owned())?;
                Err("request writer exited without reporting a result".to_owned())
            }
        }
    }

    fn receive_until_id(&self, expected_id: u64, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self
                .responses
                .recv_timeout(remaining)
                .map_err(|error| format!("waiting for response {expected_id}: {error}"))?
                .map_err(|error| format!("reading server stdout: {error}"))?;
            let message: Value = serde_json::from_slice(&line)
                .map_err(|error| format!("response is not valid JSON: {error}"))?;
            if let Some(response) = classify_message(message, Some(expected_id))? {
                return Ok(response);
            }
        }
    }

    fn finish(mut self) -> Result<(ExitStatus, Vec<u8>), String> {
        drop(self.stdin.take());
        let status = wait_for_exit(&mut self.child.0)?;
        self.stdout_reader
            .join()
            .map_err(|_| "stdout reader panicked".to_owned())?;
        let mut messages = Vec::new();
        while let Ok(line) = self.responses.try_recv() {
            let line = line.map_err(|error| format!("reading server stdout: {error}"))?;
            let message: Value = serde_json::from_slice(&line)
                .map_err(|error| format!("response is not valid JSON: {error}"))?;
            messages.push(message);
        }
        let stderr = self
            .stderr_reader
            .join()
            .map_err(|_| "stderr reader panicked".to_owned())?;
        for message in messages {
            classify_message(message, None)?;
        }
        if !status.success() {
            return Err(format!(
                "server exited with {status}; stderr: {}",
                String::from_utf8_lossy(&stderr)
            ));
        }
        Ok((status, stderr))
    }
}

#[test]
fn stdio_line_limit_drops_oversized_request_and_recovers() {
    let mut server = spawn_server();

    server
        .send_line(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"stdio-line-limit-test","version":"1.0"}}}"#.to_vec(),
            RESPONSE_TIMEOUT,
        )
        .expect("send initialize request");
    let initialize = server
        .receive_until_id(1, RESPONSE_TIMEOUT)
        .expect("receive initialize response");
    assert!(initialize.get("result").is_some());
    server
        .send_line(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_vec(),
            RESPONSE_TIMEOUT,
        )
        .expect("send initialized notification");

    let accepted = padded_ping(2, MAX_INCOMING_LINE_BYTES - 1);
    assert_eq!(accepted.len() + 1, MAX_INCOMING_LINE_BYTES);
    server
        .send_line(accepted, RESPONSE_TIMEOUT)
        .expect("send maximum-size request");
    assert_eq!(
        server
            .receive_until_id(2, RESPONSE_TIMEOUT)
            .expect("receive maximum-size response")["result"],
        json!({})
    );

    let oversized = padded_ping(3, MAX_INCOMING_LINE_BYTES);
    assert_eq!(oversized.len() + 1, MAX_INCOMING_LINE_BYTES + 1);
    server
        .send_line(oversized, RESPONSE_TIMEOUT)
        .expect("send oversized request");
    server
        .send_line(
            br#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#.to_vec(),
            RESPONSE_TIMEOUT,
        )
        .expect("send recovery request");
    assert_eq!(
        server
            .receive_until_id(4, RESPONSE_TIMEOUT)
            .expect("receive recovery response")["result"],
        json!({})
    );
    assert!(
        server
            .child
            .0
            .try_wait()
            .expect("poll live server")
            .is_none(),
        "server exited before recovering from the oversized request"
    );

    server
        .finish()
        .expect("server exits cleanly without a late oversized-request response");
}

#[test]
fn stdio_writer_timeout_kills_and_reaps_stalled_child() {
    let mut command = Command::new("sleep");
    command.arg("60");
    let mut server = spawn_command(&mut command);
    let started = Instant::now();

    let error = server
        .send_line(
            padded_ping(1, MAX_INCOMING_LINE_BYTES),
            Duration::from_millis(200),
        )
        .expect_err("a child that never reads stdin must time out");

    assert!(error.contains("timed out"), "unexpected error: {error}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stalled write did not fail promptly"
    );
    assert!(
        server
            .child
            .0
            .try_wait()
            .expect("poll stalled child")
            .is_some(),
        "stalled child was not reaped"
    );
}

#[test]
fn stdio_oracle_rejects_null_id_error() {
    let mut command = Command::new("sh");
    command.args([
        "-c",
        r#"printf '%s\n' '{"jsonrpc":"2.0","method":"fixture/ready"}' '{"jsonrpc":"2.0","id":4,"result":{}}' '{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}}'"#,
    ]);
    let server = spawn_command(&mut command);
    let response = server
        .receive_until_id(4, Duration::from_secs(1))
        .expect("receive the fixture response after its notification");
    assert_eq!(response["result"], json!({}));

    let error = server
        .finish()
        .expect_err("the final drain must reject an id:null error response");
    assert!(
        error.contains("unexpected JSON-RPC response"),
        "unexpected error: {error}"
    );
}
