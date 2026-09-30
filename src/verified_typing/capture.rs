use super::{
    channel::{valid_keycode, wait_fd},
    Stroke,
};
use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    mem,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt},
            net::UnixDatagram,
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const CAPTURE_WAIT: Duration = Duration::from_secs(5);
const CAPTURE_LIMIT: usize = 1024 * 1024;

#[derive(PartialEq, Eq)]
struct Fingerprint {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    digest: [u8; 32],
}

impl Fingerprint {
    fn read(path: &Path) -> Result<Self> {
        let file = fs::File::open(path).context("cannot open resolved typing CLI")?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() <= 16 * 1024 * 1024,
            "typing CLI is not a supported executable file"
        );
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == metadata.len(),
            "typing CLI changed while fingerprinting"
        );
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            digest: Sha256::digest(&bytes).into(),
        })
    }
}

struct Sink {
    directory: PathBuf,
    path: PathBuf,
    socket: UnixDatagram,
}

impl Sink {
    fn bind() -> Result<Self> {
        for _ in 0..8 {
            let mut random = [0; 12];
            getrandom::fill(&mut random).context("cannot generate capture socket nonce")?;
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let directory = std::env::temp_dir().join(format!("cul-strokes-{suffix}"));
            let path = directory.join("s");
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&directory) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
                Ok(()) => {}
            }
            match UnixDatagram::bind(&path).and_then(|socket| {
                socket.set_nonblocking(true)?;
                Ok(socket)
            }) {
                Ok(socket) => {
                    return Ok(Self {
                        directory,
                        path,
                        socket,
                    })
                }
                Err(error) => {
                    let _ = fs::remove_dir_all(&directory);
                    return Err(error.into());
                }
            }
        }
        bail!("cannot allocate private CLI capture socket")
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct StopCapture(Arc<AtomicBool>);
impl Drop for StopCapture {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) async fn capture(executable: &Path, text: &str) -> Result<Vec<Stroke>> {
    ensure!(
        text.len() <= 4096 && text.bytes().all(|byte| (0x20..=0x7e).contains(&byte)),
        "verified typing supports at most 4096 printable ASCII bytes"
    );
    let before = Fingerprint::read(executable)?;
    let sink = Sink::bind()?;
    let socket = sink.socket.try_clone()?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = StopCapture(stopped.clone());
    let text_length = text.len();
    let deadline = Instant::now() + CAPTURE_WAIT;
    let collection =
        tokio::task::spawn_blocking(move || collect(socket, text_length * 8, deadline, stopped));
    let mut command = tokio::process::Command::new(executable);
    command
        .args([
            "type",
            "--file",
            "-",
            "--key-delay",
            "0",
            "--key-hold",
            "0",
            "--escape",
            "0",
        ])
        .env("YDOTOOL_SOCKET", &sink.path);
    for variable in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "XAUTHORITY",
        "XDG_RUNTIME_DIR",
    ] {
        command.env_remove(variable);
    }
    let output = crate::command_runner::output_with_stdin(
        command,
        "capture verified typing CLI strokes",
        CAPTURE_WAIT,
        text.as_bytes().to_vec(),
    )
    .await;
    drop(stop);
    let packets = collection.await.context("CLI capture worker failed")??;
    let output = output?;
    ensure!(output.status.success(), "typing CLI capture failed");
    let event_bytes: usize = packets.iter().map(Vec::len).sum();
    ensure!(
        event_bytes
            .saturating_add(output.stdout.len())
            .saturating_add(output.stderr.len())
            <= CAPTURE_LIMIT,
        "typing CLI capture exceeded its output limit"
    );
    ensure!(
        before == Fingerprint::read(executable)?,
        "typing CLI changed during capture"
    );
    reduce(&packets, text.len())
}

fn collect(
    socket: UnixDatagram,
    maximum_packets: usize,
    deadline: Instant,
    stopped: Arc<AtomicBool>,
) -> Result<Vec<Vec<u8>>> {
    let mut packets = Vec::new();
    let mut total_bytes = 0;
    // A packet longer than this is rejected before allocation; the overall cap is separate.
    let mut buffer = [0; 4096];
    loop {
        let received = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_DONTWAIT | libc::MSG_TRUNC,
            )
        };
        if received >= 0 {
            let size = received as usize;
            ensure!(
                size != 0 && size <= buffer.len(),
                "invalid CLI event datagram size"
            );
            total_bytes += size;
            ensure!(
                packets.len() < maximum_packets && total_bytes <= CAPTURE_LIMIT,
                "CLI capture exceeded its output limit"
            );
            packets.push(buffer[..size].to_vec());
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        if stopped.load(Ordering::Acquire) {
            return Ok(packets);
        }
        ensure!(Instant::now() < deadline, "CLI capture deadline expired");
        match wait_fd(
            socket.as_raw_fd(),
            libc::POLLIN,
            deadline.min(Instant::now() + Duration::from_millis(10)),
        ) {
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            result => result?,
        }
    }
}

fn reduce(packets: &[Vec<u8>], characters: usize) -> Result<Vec<Stroke>> {
    let header = mem::offset_of!(libc::input_event, type_);
    let event_size = mem::size_of::<libc::input_event>();
    let mut shift = false;
    let mut pressed = None;
    let mut awaiting_sync = false;
    let mut strokes = Vec::with_capacity(characters);
    for packet in packets {
        ensure!(
            !packet.is_empty() && packet.len().is_multiple_of(event_size),
            "CLI emitted a truncated native input_event"
        );
        for event in packet.chunks_exact(event_size) {
            let kind = u16::from_ne_bytes(event[header..header + 2].try_into()?);
            let code = u16::from_ne_bytes(event[header + 2..header + 4].try_into()?);
            let value = i32::from_ne_bytes(event[header + 4..header + 8].try_into()?);
            if kind == 0 {
                ensure!(
                    code == 0 && value == 0 && awaiting_sync,
                    "CLI emitted an unexpected synchronization event"
                );
                awaiting_sync = false;
                continue;
            }
            ensure!(
                kind == 1 && !awaiting_sync && (value == 0 || value == 1),
                "CLI emitted extra event classes, repeats, or an unsynchronized key"
            );
            awaiting_sync = true;
            if code == 42 {
                ensure!(
                    pressed.is_none() && (value == 1) != shift,
                    "CLI emitted an overlapping Shift transition"
                );
                shift = value == 1;
            } else {
                ensure!(valid_keycode(code), "CLI emitted an unsupported keycode");
                if value == 1 {
                    ensure!(pressed.is_none(), "CLI emitted overlapping keys");
                    pressed = Some(Stroke {
                        keycode: code,
                        left_shift: shift,
                    });
                } else {
                    let stroke = pressed
                        .take()
                        .context("CLI released a key it did not press")?;
                    ensure!(
                        stroke.keycode == code && stroke.left_shift == shift,
                        "CLI stroke changed before release"
                    );
                    strokes.push(stroke);
                    ensure!(strokes.len() <= characters, "CLI emitted extra strokes");
                }
            }
        }
    }
    ensure!(
        !shift && pressed.is_none() && !awaiting_sync && strokes.len() == characters,
        "CLI capture did not contain exactly one released stroke per character"
    );
    Ok(strokes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn cli(mode: &str) -> (Self, PathBuf) {
            let directory = std::env::temp_dir().join(format!(
                "cul-verified-capture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let executable = directory.join("ydotool");
            let script = format!(
                r#"#!/usr/bin/python3
import os, socket, struct, sys
assert sys.argv[1:] == ['type', '--file', '-', '--key-delay', '0', '--key-hold', '0', '--escape', '0']
text = sys.stdin.buffer.read()
connection = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
connection.connect(os.environ['YDOTOOL_SOCKET'])
def event(kind, code, value):
    connection.send(struct.pack('@llHHi', 0, 0, kind, code, value))
def key(code, down):
    event(1, code, down)
    event(0, 0, 0)
mode = '{mode}'
if mode == 'normal':
    assert text == b'A'
    for code, down in [(42,1),(30,1),(30,0),(42,0)]: key(code, down)
elif mode == 'held': key(30, 1)
elif mode == 'pointer': event(2, 0, 1)
elif mode == 'repeat':
    for value in [1,2,0]: key(30, value)
elif mode == 'missing-syn': event(1,30,1)
elif mode == 'truncated': connection.send(b'bad')
elif mode == 'extra':
    for index in range(20): event(0,0,0)
elif mode == 'mutate':
    with open(__file__, 'a') as output: output.write('# modified\n')
    key(30,1); key(30,0)
elif mode == 'stdout':
    key(30,1); key(30,0)
    sys.stdout.write('x' * (1024 * 1024 + 1))
"#
            );
            fs::write(&executable, script).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            (Self(directory), executable)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn captures_the_resolved_clis_complete_shift_stroke_in_a_private_sink() {
        let (_fixture, executable) = Fixture::cli("normal");
        let strokes = capture(&executable, "A").await.expect("valid CLI capture");
        assert_eq!(
            strokes,
            vec![Stroke {
                keycode: 30,
                left_shift: true
            }]
        );
    }

    #[tokio::test]
    async fn refuses_incomplete_extra_and_changed_cli_output() {
        for mode in [
            "held",
            "pointer",
            "repeat",
            "missing-syn",
            "truncated",
            "extra",
            "mutate",
            "stdout",
        ] {
            let (_fixture, executable) = Fixture::cli(mode);
            assert!(capture(&executable, "a").await.is_err(), "mode {mode}");
        }
    }

    #[tokio::test]
    #[ignore = "requires an explicitly selected real ydotool CLI; capture uses a private sink"]
    async fn real_cli_capture_is_complete_and_never_uses_the_daemon_socket() {
        let executable = std::env::var_os("COMPUTER_USE_TEST_YDOTOOL").expect("explicit CLI path");
        let strokes = capture(Path::new(&executable), "a A!")
            .await
            .expect("real CLI capture");
        assert_eq!(
            strokes,
            vec![
                Stroke {
                    keycode: 30,
                    left_shift: false
                },
                Stroke {
                    keycode: 57,
                    left_shift: false
                },
                Stroke {
                    keycode: 30,
                    left_shift: true
                },
                Stroke {
                    keycode: 2,
                    left_shift: true
                },
            ]
        );
    }

    #[tokio::test]
    #[ignore = "requires an explicitly selected real ydotool CLI; capture uses a private sink"]
    async fn real_cli_captures_the_complete_printable_ascii_request_without_held_keys() {
        let executable = std::env::var_os("COMPUTER_USE_TEST_YDOTOOL").expect("explicit CLI path");
        let text: String = (b' '..=b'~').map(char::from).collect();
        let strokes = capture(Path::new(&executable), &text)
            .await
            .expect("complete printable ASCII capture");
        assert_eq!(strokes.len(), 95);
    }
}
