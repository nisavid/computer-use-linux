//! Request-local qualification of a pinned, opt-in ydotool typing channel.
//!
//! The protocol is independently implemented from its documented byte contract.
//! Kernel acknowledgements describe submission, never application insertion.

mod capture;
mod channel;
mod x11;

use std::{
    fmt,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QualificationFailure {
    Unknown(String),
    Incompatible(String),
}

impl fmt::Display for QualificationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(detail) => {
                write!(formatter, "verified raw typing is unqualified: {detail}")
            }
            Self::Incompatible(detail) => {
                write!(formatter, "verified raw typing is incompatible: {detail}")
            }
        }
    }
}

impl std::error::Error for QualificationFailure {}

pub(crate) struct Request {
    /// Use the resolved SupportedYdotool executable, never an MCP-supplied path.
    pub(crate) executable: PathBuf,
    pub(crate) identity_socket: PathBuf,
    pub(crate) raw_socket: PathBuf,
    pub(crate) text: String,
    pub(crate) display: String,
}

pub(crate) struct Prepared {
    channel: channel::Channel,
    observer: x11::Observer,
    strokes: Vec<Stroke>,
}

#[derive(Debug)]
pub(crate) struct DispatchFailure {
    pub(crate) detail: String,
    pub(crate) may_have_submitted: bool,
}

impl fmt::Display for DispatchFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for DispatchFailure {}

/// Capture the resolved CLI and qualify a pinned channel without submitting keys.
/// The caller retains its input lock through focus, dispatch, and bounded cleanup.
/// The configured, qualified CLI is trusted operator equipment; capture is not
/// an execution sandbox for arbitrary programs.
pub(crate) async fn prepare(request: Request) -> Result<Prepared, QualificationFailure> {
    if std::env::var("XDG_SESSION_TYPE").ok().as_deref() != Some("x11")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
        return Err(QualificationFailure::Unknown(
            "an explicit native X11 session is required".to_owned(),
        ));
    }
    let strokes = capture::capture(&request.executable, &request.text)
        .await
        .map_err(|error| QualificationFailure::Unknown(error.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let channel = channel::Channel::open(&request.identity_socket, &request.raw_socket)
            .map_err(|error| QualificationFailure::Unknown(error.to_string()))?;
        let observer = x11::Observer::prepare(
            &channel.identity.input_sysname,
            &strokes,
            &request.text,
            &request.display,
        )?;
        Ok(Prepared {
            channel,
            observer,
            strokes,
        })
    })
    .await
    .map_err(|error| {
        QualificationFailure::Unknown(format!("qualification worker failed: {error}"))
    })?
}

impl Prepared {
    /// Run in a blocking worker after the caller has focused its requested target.
    /// A failure with possible submission forbids retrying through another route.
    pub(crate) fn dispatch_after_focus(
        mut self,
        expected_pid: Option<u32>,
        expected_window: Option<u32>,
        cancelled: impl Fn() -> bool,
    ) -> Result<usize, DispatchFailure> {
        let result = self.dispatch(expected_pid, expected_window, &cancelled);
        match result {
            Ok(submitted) => Ok(submitted),
            Err(error) => {
                let cleanup = self.channel.cancel();
                let detail = match cleanup {
                    Ok(()) => error.to_string(),
                    Err(cleanup) => format!("{error}; verified channel cleanup failed: {cleanup}"),
                };
                Err(DispatchFailure {
                    detail,
                    may_have_submitted: self.channel.may_have_submitted,
                })
            }
        }
    }

    fn dispatch(
        &mut self,
        expected_pid: Option<u32>,
        expected_window: Option<u32>,
        cancelled: &dyn Fn() -> bool,
    ) -> anyhow::Result<usize> {
        anyhow::ensure!(!cancelled(), "verified typing cancelled before dispatch");
        self.channel.recheck_socket()?;
        self.observer
            .recheck_after_focus(expected_pid, expected_window)?;
        let mut submitted = 0;
        for stroke in self.strokes.iter().copied() {
            anyhow::ensure!(!cancelled(), "verified typing cancelled before stroke");
            self.observer.before_stroke(stroke)?;
            let mut observation_failure = None;
            let observer = &mut self.observer;
            let result = self.channel.stroke(stroke, &mut || {
                if cancelled() {
                    return true;
                }
                match observer.poll_during_stroke(stroke) {
                    Ok(()) => false,
                    Err(error) => {
                        observation_failure = Some(error);
                        true
                    }
                }
            });
            if let Some(error) = observation_failure {
                return Err(error.into());
            }
            result?;
            submitted += 1;
            self.observer.after_stroke(stroke)?;
            let until = Instant::now() + Duration::from_millis(20);
            while Instant::now() < until {
                anyhow::ensure!(!cancelled(), "verified typing cancelled between strokes");
                // The completed stroke's events have been drained by after_stroke.
                self.observer.poll_during_stroke(stroke)?;
                std::thread::sleep(
                    until
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(5)),
                );
            }
        }
        let mut observation_failure = None;
        let observer = &mut self.observer;
        let result = self.channel.finish(&mut || {
            if cancelled() {
                return true;
            }
            match observer.poll_completion() {
                Ok(()) => false,
                Err(error) => {
                    observation_failure = Some(error);
                    true
                }
            }
        });
        if let Some(error) = observation_failure {
            return Err(error.into());
        }
        result?;
        self.observer.finish_observation()?;
        Ok(submitted)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stroke {
    pub(crate) keycode: u16,
    pub(crate) left_shift: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run only inside a disposable native-Xorg guest with owned uinput devices.
    /// The caller supplies the guest's own CLI, sockets, focus PID/XID, and text.
    /// Application readback is a separate assertion in the guest harness.
    #[tokio::test]
    #[ignore = "requires an explicitly provisioned native Xorg/uinput guest"]
    async fn native_guest_prepare_and_dispatch() {
        assert_eq!(
            std::env::var("COMPUTER_USE_TEST_NATIVE_GUEST").as_deref(),
            Ok("1")
        );
        let request = Request {
            executable: std::env::var_os("COMPUTER_USE_TEST_YDOTOOL")
                .unwrap()
                .into(),
            identity_socket: std::env::var_os("COMPUTER_USE_TEST_IDENTITY_SOCKET")
                .unwrap()
                .into(),
            raw_socket: std::env::var_os("COMPUTER_USE_TEST_RAW_SOCKET")
                .unwrap()
                .into(),
            text: std::env::var("COMPUTER_USE_TEST_TEXT").unwrap(),
            display: std::env::var("DISPLAY").unwrap(),
        };
        let count = request.text.len();
        let prepared = prepare(request)
            .await
            .unwrap_or_else(|error| panic!("prepare: {error}"));
        let pid = std::env::var("COMPUTER_USE_TEST_TARGET_PID")
            .ok()
            .map(|value| value.parse().unwrap());
        let window = std::env::var("COMPUTER_USE_TEST_TARGET_XID")
            .ok()
            .map(|value| value.parse().unwrap());
        let submitted = tokio::task::spawn_blocking(move || {
            prepared.dispatch_after_focus(pid, window, || false)
        })
        .await
        .unwrap()
        .unwrap_or_else(|error| {
            panic!(
                "dispatch: {error}; may_have_submitted={}",
                error.may_have_submitted
            )
        });
        assert_eq!(submitted, count);
    }

    #[test]
    fn automatic_raw_requires_native_session_before_accessing_cli_or_channel() {
        const MARKER: &str = "CUL_TEST_NATIVE_SESSION_CHILD";
        if std::env::var_os(MARKER).is_some() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = runtime.block_on(prepare(Request {
                executable: PathBuf::from("/nonexistent-test-typing-cli"),
                identity_socket: PathBuf::from("/nonexistent-test-identity"),
                raw_socket: PathBuf::from("/nonexistent-test-raw"),
                text: "a".to_owned(),
                display: "unsupported-test-display".to_owned(),
            }));
            match result {
                Err(QualificationFailure::Unknown(detail)) => {
                    assert_eq!(detail, "an explicit native X11 session is required")
                }
                _ => panic!("an ambiguous session reached preparation"),
            }
            return;
        }
        for (session, wayland) in [
            (None, None),
            (Some("wayland"), None),
            (Some("x11"), Some("private-wayland")),
        ] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "verified_typing::tests::automatic_raw_requires_native_session_before_accessing_cli_or_channel",
                "--nocapture",
            ])
            .env(MARKER, "1")
            .env_remove("XDG_SESSION_TYPE")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
            if let Some(session) = session {
                command.env("XDG_SESSION_TYPE", session);
            }
            if let Some(wayland) = wayland {
                command.env("WAYLAND_DISPLAY", wayland);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}
