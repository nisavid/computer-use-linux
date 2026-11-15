use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
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
    stdin: ChildStdin,
    responses: Receiver<Result<Vec<u8>, String>>,
    stdout_reader: JoinHandle<()>,
    stderr_reader: JoinHandle<Vec<u8>>,
}

fn spawn_server() -> McpServer {
    let mut child = Command::new(env!("CARGO_BIN_EXE_computer-use-linux"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn computer-use-linux MCP server");
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
        stdin,
        responses: response_rx,
        stdout_reader,
        stderr_reader,
    }
}

fn send_line(stdin: &mut ChildStdin, payload: &[u8]) {
    stdin.write_all(payload).expect("write request payload");
    stdin.write_all(b"\n").expect("write request delimiter");
    stdin.flush().expect("flush request");
}

fn padded_ping(id: u64, json_bytes: usize) -> Vec<u8> {
    let mut request = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#).into_bytes();
    assert!(request.len() <= json_bytes);
    request.resize(json_bytes, b' ');
    serde_json::from_slice::<Value>(&request).expect("padded request remains valid JSON");
    request
}

fn receive_until_id(
    responses: &Receiver<Result<Vec<u8>, String>>,
    expected_id: u64,
    observed: &mut Vec<Value>,
) -> Value {
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = responses
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("waiting for response {expected_id}: {error}"))
            .unwrap_or_else(|error| panic!("reading server stdout: {error}"));
        let response: Value = serde_json::from_slice(&line).expect("response is valid JSON");
        let matches = response["id"] == json!(expected_id);
        observed.push(response.clone());
        if matches {
            return response;
        }
    }
}

fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("poll server exit") {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill server after exit timeout");
            let status = child.wait().expect("reap timed-out server");
            panic!("server did not exit after stdin closed: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn stdio_line_limit_drops_oversized_request_and_recovers() {
    let McpServer {
        mut child,
        mut stdin,
        responses,
        stdout_reader,
        stderr_reader,
    } = spawn_server();
    let mut observed = Vec::new();

    send_line(
        &mut stdin,
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"stdio-line-limit-test","version":"1.0"}}}"#,
    );
    let initialize = receive_until_id(&responses, 1, &mut observed);
    assert!(initialize.get("result").is_some());
    send_line(
        &mut stdin,
        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );

    let accepted = padded_ping(2, MAX_INCOMING_LINE_BYTES - 1);
    assert_eq!(accepted.len() + 1, MAX_INCOMING_LINE_BYTES);
    send_line(&mut stdin, &accepted);
    drop(accepted);
    assert_eq!(
        receive_until_id(&responses, 2, &mut observed)["result"],
        json!({})
    );

    let oversized = padded_ping(3, MAX_INCOMING_LINE_BYTES);
    assert_eq!(oversized.len() + 1, MAX_INCOMING_LINE_BYTES + 1);
    send_line(&mut stdin, &oversized);
    drop(oversized);
    send_line(&mut stdin, br#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#);
    assert_eq!(
        receive_until_id(&responses, 4, &mut observed)["result"],
        json!({})
    );
    assert!(
        child.0.try_wait().expect("poll live server").is_none(),
        "server exited before recovering from the oversized request"
    );

    drop(stdin);
    let status = wait_for_exit(&mut child.0);
    stdout_reader.join().expect("join stdout reader");
    while let Ok(line) = responses.try_recv() {
        let line = line.unwrap_or_else(|error| panic!("reading server stdout: {error}"));
        observed.push(serde_json::from_slice(&line).expect("response is valid JSON"));
    }
    let stderr = stderr_reader.join().expect("join stderr reader");

    assert!(
        !observed.iter().any(|response| response["id"] == json!(3)),
        "oversized request unexpectedly received a response: {observed:?}"
    );
    assert!(
        status.success(),
        "server exited with {status}; stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
}
