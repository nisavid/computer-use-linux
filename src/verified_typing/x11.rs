//! Request-local qualification of the native Xorg/libinput keyboard profile.
//! Relevant keys must have repeat disabled. Application text requires readback.

use super::{QualificationFailure, Stroke};
use std::{
    collections::BTreeSet,
    fmt::Display,
    io::{self, IoSlice},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{
            fs::{FileTypeExt, MetadataExt},
            net::UnixStream,
        },
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        xinput::{
            self, ConnectionExt as _, DeviceType, EventMask, XIEventMask, XIGetPropertyItems,
        },
        xkb::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
        Event,
    },
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
    x11_utils::Serialize,
};

const LIMIT: Duration = Duration::from_secs(2);
// Two ordinary full XKB maps plus at most 64 bounded device properties fit
// comfortably; larger profiles require a new qualification surface.
const READ_BUDGET: usize = 1024 * 1024;
type Conn = RustConnection<BoundedStream>;

fn unknown(error: impl Display) -> QualificationFailure {
    QualificationFailure::Unknown(error.to_string())
}
fn require(condition: bool, detail: &str) -> Result<(), QualificationFailure> {
    if condition {
        Ok(())
    } else {
        Err(unknown(detail))
    }
}
macro_rules! reply {
    ($request:expr) => {
        $request.map_err(unknown)?.reply().map_err(unknown)?
    };
}
macro_rules! checked {
    ($request:expr) => {
        $request.map_err(unknown)?.check().map_err(unknown)?
    };
}

/// Setup, reply, and flush share the phase's monotonic deadline.
struct BoundedStream {
    inner: DefaultStream,
    deadline: Arc<Mutex<Instant>>,
    bytes_read: Arc<AtomicUsize>,
}
impl BoundedStream {
    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() >= *self.deadline.lock().unwrap() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "native X11 observation exceeded its deadline",
            ));
        }
        Ok(())
    }
}
impl Stream for BoundedStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        loop {
            let remaining = self
                .deadline
                .lock()
                .unwrap()
                .saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "native X11 observation exceeded its deadline",
                ));
            }
            let mut fd = libc::pollfd {
                fd: self.inner.as_raw_fd(),
                events: (if mode.readable() { libc::POLLIN } else { 0 })
                    | (if mode.writable() { libc::POLLOUT } else { 0 }),
                revents: 0,
            };
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let result = unsafe { libc::poll(&mut fd, 1, millis) };
            if result > 0 {
                if fd.revents & libc::POLLNVAL != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "native X11 socket is invalid",
                    ));
                }
                return Ok(());
            }
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return Err(io::Error::last_os_error());
            }
        }
    }
    fn read(&self, bytes: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.check_deadline()?;
        let remaining = READ_BUDGET.saturating_sub(self.bytes_read.load(Ordering::Relaxed));
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native X11 observation exceeded its byte budget",
            ));
        }
        let length = bytes.len().min(remaining);
        let count = self.inner.read(&mut bytes[..length], fds)?;
        self.bytes_read.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }
    fn write(&self, bytes: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.check_deadline()?;
        self.inner.write(bytes, fds)
    }
    fn write_vectored(
        &self,
        bytes: &[IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.check_deadline()?;
        self.inner.write_vectored(bytes, fds)
    }
}

struct Device {
    sysfs: PathBuf,
    sysfs_inode: (u64, u64),
    event_rdev: u64,
    slave: u16,
    master: u16,
    pointer: u16,
}
#[derive(Clone, PartialEq, Eq)]
struct Profile(Vec<u8>);
struct ActiveStroke {
    stroke: Stroke,
    next: usize,
}
pub(super) struct Observer {
    conn: Conn,
    deadline: Arc<Mutex<Instant>>,
    bytes_read: Arc<AtomicUsize>,
    root: u32,
    device: Device,
    strokes: Vec<Stroke>,
    text: String,
    profile: Profile,
    focus: Option<u32>,
    ancestry: Vec<u32>,
    expected_pid: Option<u32>,
    expected_window: Option<u32>,
    active: Option<ActiveStroke>,
    user_time_atom: u32,
}

impl Observer {
    pub(super) fn prepare(
        sysname: &str,
        strokes: &[Stroke],
        text: &str,
        display: &str,
    ) -> Result<Self, QualificationFailure> {
        // Reject ambiguous displays before looking at any sysfs or socket path.
        let (display_number, screen) = local_display(display)?;
        require(
            text.len() == strokes.len() && text.bytes().all(|b| (0x20..=0x7e).contains(&b)),
            "captured strokes do not describe supported printable ASCII",
        )?;
        require(
            strokes
                .iter()
                .all(|stroke| matches!(stroke.keycode, 2..=13 | 16..=27 | 30..=41 | 43..=53 | 57)),
            "captured stroke is outside the supported printable key positions",
        )?;
        require(
            sysname
                .strip_prefix("input")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
            "daemon returned an invalid input sysname",
        )?;
        let (sysfs, sysfs_inode, event_rdev) = event_identity(sysname)?;
        let socket = connect_local(display_number)?;
        let peer = peer_pid(socket.as_raw_fd())?;
        let executable = std::fs::read_link(format!("/proc/{peer}/exe")).map_err(unknown)?;
        require(
            executable.file_name().is_some_and(|name| name == "Xorg"),
            "the local display peer is not a verifiable native Xorg process",
        )?;
        let (inner, (family, address)) =
            DefaultStream::from_unix_stream(socket).map_err(unknown)?;
        let auth =
            x11rb::reexports::x11rb_protocol::xauth::get_auth(family, &address, display_number)
                .map_err(unknown)?
                .unwrap_or_default();
        let deadline = Arc::new(Mutex::new(Instant::now() + LIMIT));
        let bytes_read = Arc::new(AtomicUsize::new(0));
        let conn = RustConnection::connect_to_stream_with_auth_info(
            BoundedStream {
                inner,
                deadline: deadline.clone(),
                bytes_read: bytes_read.clone(),
            },
            screen,
            auth.0,
            auth.1,
        )
        .map_err(unknown)?;
        require(
            !reply!(conn.query_extension(b"XWAYLAND")).present,
            "XWayland is not a qualified native input surface",
        )?;
        require(
            reply!(conn.xkb_use_extension(1, 0)).supported,
            "XKB 1.0 is unavailable",
        )?;
        let xi = reply!(conn.xinput_xi_query_version(2, 0));
        require(xi.major_version >= 2, "XInput 2 is unavailable")?;
        let root = conn.setup().roots[screen].root;
        // Subscribe before discovering the device and reading the first map.
        checked!(conn.xinput_xi_select_events(
            root,
            &[
                EventMask {
                    deviceid: 0,
                    mask: vec![
                        XIEventMask::HIERARCHY
                            | XIEventMask::DEVICE_CHANGED
                            | XIEventMask::FOCUS_IN
                            | XIEventMask::FOCUS_OUT
                    ]
                },
                EventMask {
                    deviceid: 1,
                    mask: vec![XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_KEY_RELEASE]
                },
            ]
        ));
        let (slave, master, pointer) = associate(&conn, event_rdev, strokes)?;
        for id in [slave, master] {
            checked!(conn.xkb_select_events(
                id,
                xkb::EventType::default(),
                xkb::EventType::NEW_KEYBOARD_NOTIFY
                    | xkb::EventType::MAP_NOTIFY
                    | xkb::EventType::STATE_NOTIFY
                    | xkb::EventType::CONTROLS_NOTIFY
                    | xkb::EventType::NAMES_NOTIFY
                    | xkb::EventType::COMPAT_MAP_NOTIFY
                    | xkb::EventType::ACCESS_X_NOTIFY
                    | xkb::EventType::INDICATOR_MAP_NOTIFY,
                all_map_parts(),
                all_map_parts(),
                &xkb::SelectEventsAux::default()
            ));
        }
        let device = Device {
            sysfs,
            sysfs_inode,
            event_rdev,
            slave,
            master,
            pointer,
        };
        let profile = read_profile(&conn, &device, strokes, text)?;
        neutral(&conn, &device)?;
        let user_time_atom = reply!(conn.intern_atom(false, b"_NET_WM_USER_TIME")).atom;
        require(
            user_time_atom != 0,
            "the activity timestamp atom is invalid",
        )?;
        let mut observer = Self {
            conn,
            deadline,
            bytes_read,
            root,
            device,
            strokes: strokes.to_vec(),
            text: text.to_owned(),
            profile,
            focus: None,
            ancestry: Vec::new(),
            expected_pid: None,
            expected_window: None,
            active: None,
            user_time_atom,
        };
        observer.barrier()?;
        observer.drain()?;
        Ok(observer)
    }

    pub(super) fn recheck_after_focus(
        &mut self,
        expected_pid: Option<u32>,
        expected_window: Option<u32>,
    ) -> Result<(), QualificationFailure> {
        self.phase();
        self.expected_pid = expected_pid;
        self.expected_window = expected_window;
        self.drain()?;
        self.recheck_profile()?;
        neutral(&self.conn, &self.device)?;
        let focus = self.current_focus()?;
        let ancestry = self.check_target(focus)?;
        for &window in &ancestry {
            checked!(self.conn.xinput_xi_select_events(
                window,
                &[
                    EventMask {
                        deviceid: self.device.master,
                        mask: vec![XIEventMask::FOCUS_IN | XIEventMask::FOCUS_OUT]
                    },
                    EventMask {
                        deviceid: self.device.slave,
                        mask: vec![XIEventMask::FOCUS_IN | XIEventMask::FOCUS_OUT]
                    }
                ]
            ));
            checked!(self.conn.change_window_attributes(
                window,
                &xproto::ChangeWindowAttributesAux::new().event_mask(
                    xproto::EventMask::PROPERTY_CHANGE | xproto::EventMask::STRUCTURE_NOTIFY
                )
            ));
        }
        self.focus = Some(focus);
        self.ancestry = ancestry;
        self.barrier()?;
        self.drain()?;
        self.check_focus()?;
        Ok(())
    }

    pub(super) fn before_stroke(&mut self, stroke: Stroke) -> Result<(), QualificationFailure> {
        self.phase();
        require(
            self.active.is_none(),
            "a prior observed stroke is incomplete",
        )?;
        self.drain()?;
        self.recheck_profile()?;
        neutral(&self.conn, &self.device)?;
        self.check_focus()?;
        self.barrier()?;
        self.drain()?;
        self.active = Some(ActiveStroke { stroke, next: 0 });
        Ok(())
    }

    pub(super) fn poll_during_stroke(&mut self, _: Stroke) -> Result<(), QualificationFailure> {
        self.phase();
        self.drain()?;
        self.check_focus()?;
        Ok(())
    }

    pub(super) fn after_stroke(&mut self, stroke: Stroke) -> Result<(), QualificationFailure> {
        self.phase();
        let until = Instant::now() + LIMIT;
        loop {
            self.drain()?;
            self.check_focus()?;
            if self.active.as_ref().is_some_and(|active| {
                active.stroke == stroke && active.next == if stroke.left_shift { 4 } else { 2 }
            }) {
                break;
            }
            require(
                Instant::now() < until,
                "XInput did not observe the complete acknowledged stroke",
            )?;
            self.barrier()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        self.recheck_profile()?;
        neutral(&self.conn, &self.device)?;
        self.barrier()?;
        self.drain()?;
        self.active = None;
        Ok(())
    }

    pub(super) fn poll_completion(&mut self) -> Result<(), QualificationFailure> {
        require(
            self.active.is_none(),
            "a submitted stroke remains unobserved at completion",
        )?;
        self.phase();
        self.drain()?;
        self.check_focus()
    }

    pub(super) fn finish_observation(&mut self) -> Result<(), QualificationFailure> {
        self.poll_completion()?;
        self.recheck_profile()?;
        neutral(&self.conn, &self.device)?;
        self.barrier()?;
        self.drain()?;
        self.check_focus()
    }

    fn phase(&self) {
        *self.deadline.lock().unwrap() = Instant::now() + LIMIT;
        self.bytes_read.store(0, Ordering::Relaxed);
    }
    fn barrier(&self) -> Result<(), QualificationFailure> {
        reply!(self.conn.get_input_focus());
        Ok(())
    }
    fn recheck_profile(&self) -> Result<(), QualificationFailure> {
        let metadata = std::fs::metadata(&self.device.sysfs).map_err(unknown)?;
        require(
            (metadata.dev(), metadata.ino()) == self.device.sysfs_inode,
            "the daemon input device was replaced",
        )?;
        let (_, _, current_rdev) = event_identity(
            self.device
                .sysfs
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| unknown("input sysname vanished"))?,
        )?;
        require(
            current_rdev == self.device.event_rdev,
            "the daemon event node changed",
        )?;
        let ids = associate(&self.conn, self.device.event_rdev, &self.strokes)?;
        require(
            ids == (self.device.slave, self.device.master, self.device.pointer),
            "the daemon XInput association changed",
        )?;
        require(
            read_profile(&self.conn, &self.device, &self.strokes, &self.text)? == self.profile,
            "the qualified XKB profile changed",
        )
    }
    fn current_focus(&self) -> Result<u32, QualificationFailure> {
        let core = reply!(self.conn.get_input_focus()).focus;
        let master = reply!(self.conn.xinput_xi_get_focus(self.device.master)).focus;
        let slave = reply!(self.conn.xinput_xi_get_focus(self.device.slave)).focus;
        require(
            // Xorg returns the slave's focus sentinel unchanged. None has no
            // independent slave delivery; a concrete equal focus stays scoped.
            // PointerRoot routes to the sprite window and remains unqualified.
            core > 1 && core != self.root && master == core && (slave == 0 || slave == core),
            "the associated slave/master focus is outside the supported delivery profile",
        )?;
        Ok(core)
    }
    fn check_target(&self, focus: u32) -> Result<Vec<u32>, QualificationFailure> {
        let atom = reply!(self.conn.intern_atom(false, b"_NET_WM_PID")).atom;
        let mut ancestry = Vec::new();
        let mut window = focus;
        let mut owner = None;
        for _ in 0..64 {
            require(
                window != 0 && !ancestry.contains(&window),
                "focused window ancestry is invalid",
            )?;
            ancestry.push(window);
            let pid = reply!(self.conn.get_property(
                false,
                window,
                atom,
                xproto::AtomEnum::CARDINAL,
                0,
                1
            ));
            if pid.type_ != 0 {
                require(
                    pid.type_ == u32::from(xproto::AtomEnum::CARDINAL)
                        && pid.format == 32
                        && pid.bytes_after == 0
                        && pid.value_len == 1,
                    "focused window PID property is ambiguous",
                )?;
                let value = pid
                    .value32()
                    .and_then(|mut values| values.next())
                    .ok_or_else(|| unknown("focused window PID is unavailable"))?;
                require(
                    value > 0 && owner.is_none_or(|known| known == value),
                    "focused window ancestry has conflicting owners",
                )?;
                owner = Some(value);
            }
            if window == self.root {
                break;
            }
            window = reply!(self.conn.query_tree(window)).parent;
        }
        require(
            ancestry.last() == Some(&self.root),
            "focused window ancestry exceeds its bound",
        )?;
        if let Some(pid) = self.expected_pid {
            require(
                owner == Some(pid),
                "focused window owner does not match the requested target PID",
            )?;
        }
        if let Some(window) = self.expected_window {
            require(
                ancestry.contains(&window),
                "focused window is outside the requested native X11 window",
            )?;
        }
        Ok(ancestry)
    }
    fn check_focus(&self) -> Result<(), QualificationFailure> {
        let focus = self.current_focus()?;
        require(
            self.focus == Some(focus),
            "native X11 focus changed during verified typing",
        )?;
        require(
            self.check_target(focus)? == self.ancestry,
            "focused window ancestry changed during verified typing",
        )
    }
    fn drain(&mut self) -> Result<(), QualificationFailure> {
        for _ in 0..256 {
            let Some(event) = self.conn.poll_for_event().map_err(unknown)? else {
                return Ok(());
            };
            match event {
                Event::XinputRawKeyPress(event) => self.raw_event(
                    event.deviceid,
                    event.sourceid,
                    event.detail,
                    true,
                    u32::from(event.flags),
                    event.response_type,
                )?,
                Event::XinputRawKeyRelease(event) => self.raw_event(
                    event.deviceid,
                    event.sourceid,
                    event.detail,
                    false,
                    u32::from(event.flags),
                    event.response_type,
                )?,
                Event::XkbStateNotify(event) => {
                    let shift = self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.stroke.left_shift);
                    require(
                        [self.device.slave, self.device.master].contains(&(event.device_id as u16))
                            && u16::from(event.latched_mods) == 0
                            && u16::from(event.locked_mods) == 0
                            && u8::from(event.group) == 0
                            && event.base_group == 0
                            && event.latched_group == 0
                            && u8::from(event.locked_group) == 0
                            && (u16::from(event.mods) == 0
                                || (shift && u16::from(event.mods) == 1))
                            && (u16::from(event.base_mods) == 0
                                || (shift && u16::from(event.base_mods) == 1))
                            && u16::from(event.ptr_btn_state) == 0,
                        "unexpected XKB modifier, lock, group, or pointer state",
                    )?;
                }
                Event::XinputFocusIn(_) | Event::XinputFocusOut(_) if self.focus.is_none() => {}
                // Active clients update this activity timestamp on keypress.
                // Actual owner, focus, structure, or map changes still refuse.
                Event::PropertyNotify(event)
                    if event.response_type == xproto::PROPERTY_NOTIFY_EVENT
                        && event.atom == self.user_time_atom
                        && event.state == xproto::Property::NEW_VALUE
                        && self.ancestry.contains(&event.window) => {}
                _ => return Err(unknown(
                    "native input, map, hierarchy, focus, or target changed during qualification",
                )),
            }
        }
        Err(unknown("native X11 event collection exceeded its bound"))
    }
    fn raw_event(
        &mut self,
        device: u16,
        source: u16,
        detail: u32,
        press: bool,
        flags: u32,
        response: u8,
    ) -> Result<(), QualificationFailure> {
        require(
            device == self.device.master
                && source == self.device.slave
                && flags == 0
                && response & 0x80 == 0,
            "unexpected or repeated keyboard input during verified typing",
        )?;
        let active = self
            .active
            .as_mut()
            .ok_or_else(|| unknown("keyboard input occurred outside the submitted stroke"))?;
        let key = u32::from(active.stroke.keycode) + 8;
        let sequence = if active.stroke.left_shift {
            vec![(50, true), (key, true), (key, false), (50, false)]
        } else {
            vec![(key, true), (key, false)]
        };
        require(
            sequence.get(active.next) == Some(&(detail, press)),
            "XInput observed an unexpected, duplicate, or out-of-order stroke",
        )?;
        active.next += 1;
        Ok(())
    }
}

fn local_display(display: &str) -> Result<(u16, usize), QualificationFailure> {
    let suffix = display
        .strip_prefix("unix/:")
        .or_else(|| display.strip_prefix(':'))
        .ok_or_else(|| unknown("an explicit local Unix display is required"))?;
    let mut parts = suffix.split('.');
    let number = parts.next().unwrap_or_default();
    let screen = parts.next().unwrap_or("0");
    require(
        !number.is_empty()
            && !screen.is_empty()
            && parts.next().is_none()
            && number.bytes().all(|b| b.is_ascii_digit())
            && screen.bytes().all(|b| b.is_ascii_digit()),
        "an explicit local Unix display is required",
    )?;
    Ok((
        number
            .parse()
            .map_err(|_| unknown("an explicit local Unix display is required"))?,
        screen
            .parse()
            .map_err(|_| unknown("an explicit local Unix display is required"))?,
    ))
}
fn connect_local(number: u16) -> Result<UnixStream, QualificationFailure> {
    let path = format!("/tmp/.X11-unix/X{number}");
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(unknown(io::Error::last_os_error()));
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(path.as_bytes()) {
        *target = *source as libc::c_char;
    }
    let result = unsafe {
        libc::connect(
            fd,
            &address as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&address) as libc::socklen_t,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(libc::EINPROGRESS) | Some(libc::EAGAIN)
        ) {
            return Err(unknown(error));
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        require(
            unsafe { libc::poll(&mut poll, 1, 2000) } > 0,
            "native X11 connection exceeded its deadline",
        )?;
        let mut error = 0i32;
        let mut size = std::mem::size_of_val(&error) as libc::socklen_t;
        require(
            unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    &mut error as *mut _ as *mut _,
                    &mut size,
                )
            } == 0
                && error == 0,
            "native X11 connection failed",
        )?;
    }
    Ok(stream)
}
fn peer_pid(fd: RawFd) -> Result<i32, QualificationFailure> {
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of_val(&peer) as libc::socklen_t;
    require(
        unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut peer as *mut _ as *mut _,
                &mut size,
            )
        } == 0
            && peer.pid > 0
            && size as usize == std::mem::size_of_val(&peer),
        "native X11 peer credentials are unavailable",
    )?;
    let ours = std::fs::metadata("/proc/self/ns/pid").map_err(unknown)?;
    let theirs = std::fs::metadata(format!("/proc/{}/ns/pid", peer.pid)).map_err(unknown)?;
    require(
        (ours.dev(), ours.ino()) == (theirs.dev(), theirs.ino()),
        "native X11 peer PID namespace is ambiguous",
    )?;
    Ok(peer.pid)
}
fn event_identity(sysname: &str) -> Result<(PathBuf, (u64, u64), u64), QualificationFailure> {
    let path = PathBuf::from("/sys/class/input").join(sysname);
    let metadata = std::fs::metadata(&path).map_err(unknown)?;
    let events: Vec<_> = std::fs::read_dir(&path)
        .map_err(unknown)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.strip_prefix("event")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
        })
        .collect();
    require(
        events.len() == 1,
        "daemon sysfs identity does not have exactly one event node",
    )?;
    let dev = std::fs::read_to_string(events[0].path().join("dev")).map_err(unknown)?;
    let (major, minor) = dev
        .trim()
        .split_once(':')
        .ok_or_else(|| unknown("event device major/minor is invalid"))?;
    let major: u32 = major.parse().map_err(unknown)?;
    let minor: u32 = minor.parse().map_err(unknown)?;
    let node =
        std::fs::metadata(Path::new("/dev/input").join(events[0].file_name())).map_err(unknown)?;
    require(
        node.file_type().is_char_device() && node.rdev() == libc::makedev(major, minor),
        "event node does not match daemon sysfs major/minor",
    )?;
    Ok((path, (metadata.dev(), metadata.ino()), node.rdev()))
}
fn associate(
    conn: &Conn,
    rdev: u64,
    strokes: &[Stroke],
) -> Result<(u16, u16, u16), QualificationFailure> {
    let devices = reply!(conn.xinput_xi_query_device(0u16)).infos;
    require(
        devices.len() <= 64,
        "XInput device discovery exceeded its bound",
    )?;
    let atom = reply!(conn.intern_atom(true, b"Device Node")).atom;
    require(atom != 0, "XInput Device Node property is unavailable")?;
    let mut matches = Vec::new();
    for device in &devices {
        if device.type_ != DeviceType::SLAVE_KEYBOARD || !device.enabled {
            continue;
        }
        let property = reply!(conn.xinput_xi_get_property(
            device.deviceid,
            false,
            atom,
            xproto::AtomEnum::STRING.into(),
            0,
            1024
        ));
        if property.type_ == 0 {
            continue;
        }
        require(
            property.type_ == u32::from(xproto::AtomEnum::STRING) && property.bytes_after == 0,
            "XInput Device Node property is ambiguous",
        )?;
        let XIGetPropertyItems::Data8(mut bytes) = property.items else {
            return Err(unknown("XInput Device Node is not a string"));
        };
        if bytes.last() == Some(&0) {
            bytes.pop();
        }
        require(
            !bytes.contains(&0),
            "XInput Device Node has embedded NUL bytes",
        )?;
        let path = std::str::from_utf8(&bytes).map_err(unknown)?;
        require(
            path.starts_with("/dev/input/event") && !path.contains("/../"),
            "XInput Device Node is outside the supported input path",
        )?;
        let metadata = std::fs::metadata(path).map_err(unknown)?;
        if metadata.file_type().is_char_device() && metadata.rdev() == rdev {
            matches.push(device);
        }
    }
    require(
        matches.len() == 1,
        "daemon event node is not associated with exactly one enabled XInput slave keyboard",
    )?;
    let slave = matches[0];
    require(
        slave.deviceid < 128,
        "XInput slave cannot be inspected through the supported device-state interface",
    )?;
    let mode_atom = reply!(conn.intern_atom(true, b"libinput Send Events Mode Enabled")).atom;
    require(
        mode_atom != 0,
        "the associated keyboard has no qualified libinput mode property",
    )?;
    let mode = reply!(conn.xinput_xi_get_property(
        slave.deviceid,
        false,
        mode_atom,
        xproto::AtomEnum::INTEGER.into(),
        0,
        1
    ));
    require(
        mode.type_ == u32::from(xproto::AtomEnum::INTEGER)
            && mode.bytes_after == 0
            && matches!(&mode.items, XIGetPropertyItems::Data8(bytes) if bytes.as_slice() == [0, 0]),
        "the associated keyboard is outside the enabled libinput profile",
    )?;
    let master = devices
        .iter()
        .find(|device| {
            device.deviceid == slave.attachment
                && device.enabled
                && device.type_ == DeviceType::MASTER_KEYBOARD
        })
        .ok_or_else(|| unknown("daemon slave has no enabled master keyboard"))?;
    for keyboard in [slave, master] {
        let keys: Vec<_> = keyboard
            .classes
            .iter()
            .filter_map(|class| match &class.data {
                xinput::DeviceClassData::Key(keys) => Some(keys),
                _ => None,
            })
            .collect();
        require(
            keys.len() == 1
                && strokes.iter().all(|stroke| {
                    keys[0].keys.contains(&(u32::from(stroke.keycode) + 8))
                        && (!stroke.left_shift || keys[0].keys.contains(&50))
                }),
            "associated XInput keyboard KeyClass does not cover the captured strokes",
        )?;
    }
    let pointer = devices
        .iter()
        .find(|device| {
            device.deviceid == master.attachment
                && device.enabled
                && device.type_ == DeviceType::MASTER_POINTER
                && device.attachment == master.deviceid
        })
        .ok_or_else(|| unknown("daemon master has no paired pointer"))?;
    // Establish the core query's implicit client pointer before inspecting it.
    reply!(conn.get_input_focus());
    let client = reply!(conn.xinput_xi_get_client_pointer(0));
    require(
        client.set && client.deviceid == pointer.deviceid,
        "core X11 queries are bound to a different master keyboard",
    )?;
    Ok((slave.deviceid, master.deviceid, pointer.deviceid))
}
fn all_map_parts() -> xkb::MapPart {
    xkb::MapPart::KEY_TYPES
        | xkb::MapPart::KEY_SYMS
        | xkb::MapPart::KEY_ACTIONS
        | xkb::MapPart::KEY_BEHAVIORS
        | xkb::MapPart::VIRTUAL_MODS
        | xkb::MapPart::EXPLICIT_COMPONENTS
        | xkb::MapPart::MODIFIER_MAP
        | xkb::MapPart::VIRTUAL_MOD_MAP
}
fn read_profile(
    conn: &Conn,
    device: &Device,
    strokes: &[Stroke],
    text: &str,
) -> Result<Profile, QualificationFailure> {
    let mut fingerprint = Vec::new();
    let mut interpretation = None;
    for id in [device.slave, device.master] {
        let mut map = reply!(conn.xkb_get_map(
            id,
            all_map_parts(),
            xkb::MapPart::default(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            xkb::VMod::default(),
            0,
            0,
            0,
            0,
            0,
            0
        ));
        let mut names = reply!(conn.xkb_get_names(id, xkb::NameDetail::KEY_NAMES));
        let mut controls = reply!(conn.xkb_get_controls(id));
        require(
            controls.num_groups == 1
                && u32::from(controls.enabled_controls) & !u32::from(xkb::BoolCtrl::REPEAT_KEYS)
                    == 0,
            "XKB groups or accessibility controls are outside the supported profile",
        )?;
        require(
            u16::from(controls.internal_mods_mask) == 0
                && u16::from(controls.ignore_lock_mods_mask) == 0
                && u16::from(controls.internal_mods_real_mods) == 0
                && u16::from(controls.ignore_lock_mods_real_mods) == 0
                && u16::from(controls.internal_mods_vmods) == 0
                && u16::from(controls.ignore_lock_mods_vmods) == 0,
            "XKB internal modifiers are outside the supported profile",
        )?;
        let symbols = interpret(&map, &names, &controls, strokes)?;
        if let Some(previous) = &interpretation {
            require(
                previous == &symbols,
                "slave and master XKB interpretations differ",
            )?;
        }
        for (actual, expected) in symbols.iter().zip(text.bytes()) {
            if *actual != u32::from(expected) {
                return Err(QualificationFailure::Incompatible("captured CLI strokes do not produce the requested characters on the associated XKB map".to_owned()));
            }
        }
        interpretation = Some(symbols);
        map.sequence = 0;
        names.sequence = 0;
        controls.sequence = 0;
        fingerprint.extend(map.serialize());
        fingerprint.extend(names.serialize());
        fingerprint.extend(controls.serialize());
    }
    Ok(Profile(fingerprint))
}

fn interpret(
    map: &xkb::GetMapReply,
    names: &xkb::GetNamesReply,
    controls: &xkb::GetControlsReply,
    strokes: &[Stroke],
) -> Result<Vec<u32>, QualificationFailure> {
    let symbols = map
        .map
        .syms_rtrn
        .as_ref()
        .ok_or_else(|| unknown("XKB symbols are unavailable"))?;
    require(
        !symbols.iter().flat_map(|key| &key.syms).any(|symbol| {
            (0xfe50..=0xfe6f).contains(symbol)
                || (0xfe80..=0xfe8d).contains(symbol)
                || (0xfe90..=0xfe93).contains(symbol)
                || *symbol == 0xff20
        }),
        "the reachable XKB map contains dead or Compose symbols",
    )?;
    let mut relevant: BTreeSet<u8> = strokes
        .iter()
        .map(|stroke| (stroke.keycode + 8) as u8)
        .collect();
    if strokes.iter().any(|stroke| stroke.left_shift) {
        relevant.insert(50);
    }
    for key in relevant {
        require(
            controls.per_key_repeat[key as usize / 8] & (1 << (key % 8)) == 0,
            "repeat must be disabled for every key used by verified raw typing",
        )?;
        let physical = names
            .value_list
            .key_names
            .as_ref()
            .and_then(|keys| {
                key.checked_sub(names.first_key)
                    .and_then(|offset| keys.get(offset as usize))
            })
            .ok_or_else(|| unknown("XKB physical key name is unavailable"))?;
        require(
            physical.name == physical_name(u16::from(key - 8))?,
            "XKB key names do not match the qualified evdev offset",
        )?;
        require(
            !map.map
                .behaviors_rtrn
                .as_ref()
                .ok_or_else(|| unknown("XKB behaviors are unavailable"))?
                .iter()
                .any(|behavior| behavior.keycode == key && behavior.behavior.serialize()[0] != 0),
            "XKB key has an unsupported behavior",
        )?;
        require(
            !map.map
                .vmodmap_rtrn
                .as_ref()
                .ok_or_else(|| unknown("XKB virtual modifier map is unavailable"))?
                .iter()
                .any(|entry| entry.keycode == key && u16::from(entry.vmods) != 0),
            "XKB key has virtual modifiers",
        )?;
        let modifier = map
            .map
            .modmap_rtrn
            .as_ref()
            .ok_or_else(|| unknown("XKB modifier map is unavailable"))?
            .iter()
            .find(|entry| entry.keycode == key)
            .map(|entry| u16::from(entry.mods))
            .unwrap_or(0);
        require(
            modifier == if key == 50 { 1 } else { 0 },
            "XKB printable or Shift key has an unexpected modifier map",
        )?;
        let actions = map
            .map
            .key_actions
            .as_ref()
            .ok_or_else(|| unknown("XKB key actions are unavailable"))?;
        let index = key
            .checked_sub(map.first_key_action)
            .map(usize::from)
            .ok_or_else(|| unknown("XKB key action range is incomplete"))?;
        let count = *actions
            .acts_rtrn_count
            .get(index)
            .ok_or_else(|| unknown("XKB key action range is incomplete"))?
            as usize;
        let start: usize = actions.acts_rtrn_count[..index]
            .iter()
            .map(|count| *count as usize)
            .sum();
        let actions = actions
            .acts_rtrn_acts
            .get(start..start + count)
            .ok_or_else(|| unknown("XKB actions are incomplete"))?;
        if key == 50 {
            let shift_symbols = symbols
                .get(
                    key.checked_sub(map.first_key_sym)
                        .ok_or_else(|| unknown("XKB Shift symbol range is incomplete"))?
                        as usize,
                )
                .ok_or_else(|| unknown("physical LeftShift symbols are unavailable"))?;
            require(
                shift_symbols.group_info == 1
                    && shift_symbols.width == 1
                    && shift_symbols.syms.as_slice() == [0xffe1],
                "physical LeftShift must produce only the Shift_L keysym",
            )?;
            let shift_type = map
                .map
                .types_rtrn
                .as_ref()
                .and_then(|types| {
                    shift_symbols.kt_index[0]
                        .checked_sub(map.first_type)
                        .and_then(|index| types.get(index as usize))
                })
                .ok_or_else(|| unknown("physical LeftShift type is unavailable"))?;
            require(
                shift_type.num_levels == 1
                    && u16::from(shift_type.mods_mask) == 0
                    && u16::from(shift_type.mods_mods) == 0
                    && u16::from(shift_type.mods_vmods) == 0
                    && shift_type.map.is_empty()
                    && !shift_type.has_preserve
                    && shift_type.preserve.is_empty(),
                "physical LeftShift must have an ordinary one-level type",
            )?;
            require(
                !actions.is_empty(),
                "physical LeftShift has no explicit supported action",
            )?;
            for action in actions {
                let bytes = action.serialize();
                require(
                    bytes[0] == 1
                        && bytes[1] & !5 == 0
                        && bytes[2] == 1
                        && bytes[3] == 1
                        && bytes[4..].iter().all(|b| *b == 0),
                    "physical LeftShift does not have an ordinary Shift-only SetMods action",
                )?;
            }
        } else {
            require(
                actions.iter().all(|action| action.serialize() == [0; 8]),
                "printable key has an unsupported XKB action",
            )?;
        }
    }
    strokes
        .iter()
        .map(|stroke| {
            let code = u8::try_from(stroke.keycode + 8).map_err(unknown)?;
            let key = symbols
                .get(
                    code.checked_sub(map.first_key_sym)
                        .ok_or_else(|| unknown("XKB symbol range is incomplete"))?
                        as usize,
                )
                .ok_or_else(|| unknown("XKB symbol range is incomplete"))?;
            require(
                key.group_info == 1
                    && (1..=2).contains(&key.width)
                    && key.syms.len() == key.width as usize,
                "XKB key does not have exactly one ordinary symbol group",
            )?;
            let types = map
                .map
                .types_rtrn
                .as_ref()
                .ok_or_else(|| unknown("XKB types are unavailable"))?;
            let key_type = types
                .get(
                    key.kt_index[0]
                        .checked_sub(map.first_type)
                        .ok_or_else(|| unknown("XKB type range is incomplete"))?
                        as usize,
                )
                .ok_or_else(|| unknown("XKB key type is unavailable"))?;
            let mask = u16::from(key_type.mods_mask);
            require(
                u16::from(key_type.mods_vmods) == 0
                    && u16::from(key_type.mods_mods) == mask
                    && ((key_type.num_levels == 1 && mask == 0)
                        || (key_type.num_levels == 2 && (mask == 1 || mask == 3)))
                    && key_type.num_levels == key.width
                    && key_type.preserve.iter().all(|preserve| {
                        u16::from(preserve.mask) == 0
                            && u16::from(preserve.real_mods) == 0
                            && u16::from(preserve.vmods) == 0
                    }),
                "XKB key type is outside the supported one/two-level profile",
            )?;
            let mods = if stroke.left_shift { 1 } else { 0 };
            let mut level = None;
            for entry in &key_type.map {
                require(
                    entry.active
                        && u16::from(entry.mods_vmods) == 0
                        && u16::from(entry.mods_mask) == u16::from(entry.mods_mods)
                        && u16::from(entry.mods_mask) & !mask == 0
                        && entry.level < key_type.num_levels,
                    "XKB type entry is unsupported",
                )?;
                if u16::from(entry.mods_mask) == mods & mask {
                    require(level.is_none(), "XKB type has ambiguous matching entries")?;
                    level = Some(entry.level);
                }
            }
            key.syms
                .get(level.unwrap_or(0) as usize)
                .copied()
                .ok_or_else(|| unknown("XKB selected symbol is unavailable"))
        })
        .collect()
}

// Physical rows follow Linux input-event-codes.h and Xorg's evdev offset.
// These are positions, not ydotool's character table.
fn physical_name(code: u16) -> Result<[u8; 4], QualificationFailure> {
    let row = match code {
        2..=13 => format!("AE{:02}", code - 1),
        16..=27 => format!("AD{:02}", code - 15),
        30..=40 => format!("AC{:02}", code - 29),
        41 => "TLDE".to_owned(),
        42 => "LFSH".to_owned(),
        43 => "BKSL".to_owned(),
        44..=53 => format!("AB{:02}", code - 43),
        57 => "SPCE".to_owned(),
        _ => {
            return Err(unknown(
                "captured key is outside the qualified evdev positions",
            ))
        }
    };
    Ok(row.as_bytes().try_into().unwrap())
}
fn neutral(conn: &Conn, device: &Device) -> Result<(), QualificationFailure> {
    require(
        reply!(conn.query_keymap()).keys == [0; 32],
        "core keyboard has held keys",
    )?;
    for id in [device.slave, device.master] {
        let state = reply!(conn.xkb_get_state(id));
        require(
            u16::from(state.mods) == 0
                && u16::from(state.base_mods) == 0
                && u16::from(state.latched_mods) == 0
                && u16::from(state.locked_mods) == 0
                && u8::from(state.group) == 0
                && u8::from(state.locked_group) == 0
                && state.base_group == 0
                && state.latched_group == 0
                && u16::from(state.compat_state) == 0
                && u16::from(state.lookup_mods) == 0
                && u16::from(state.compat_lookup_mods) == 0
                && u16::from(state.grab_mods) == 0
                && u16::from(state.compat_grab_mods) == 0
                && u16::from(state.ptr_btn_state) == 0,
            "associated keyboard has modifiers, locks, group state, or pointer buttons",
        )?;
        require(
            id < 128,
            "associated keyboard device state is outside the supported XI1 range",
        )?;
        let held = reply!(conn.xinput_query_device_state(id as u8));
        let keys: Vec<_> = held
            .classes
            .iter()
            .filter_map(|class| match &class.data {
                xinput::InputStateData::Key(keys) => Some(keys),
                _ => None,
            })
            .collect();
        require(
            keys.len() == 1 && keys[0].keys == [0; 32],
            "associated keyboard held-key state is unavailable or non-neutral",
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "x11_fixture.rs"]
mod wire_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn qualification_rejects_remote_or_implicit_display_without_connecting() {
        for display in [
            "",
            "localhost:0",
            "127.0.0.1:0",
            "host:1",
            ":0/../../",
            ":0.",
            ":0.1.2",
            ":65536",
        ] {
            match Observer::prepare(
                "input1",
                &[Stroke {
                    keycode: 30,
                    left_shift: false,
                }],
                "a",
                display,
            ) {
                Err(QualificationFailure::Unknown(detail)) => {
                    assert!(detail.contains("explicit local Unix"), "{detail}")
                }
                _ => panic!("an unsupported display qualified"),
            }
        }
    }
    #[test]
    fn qualification_rejects_invalid_device_identity_before_connecting() {
        for name in [
            "",
            "../input1",
            "input1/../../",
            "input",
            "event1",
            "input-1",
        ] {
            match Observer::prepare(
                name,
                &[Stroke {
                    keycode: 30,
                    left_shift: false,
                }],
                "a",
                ":65000",
            ) {
                Err(QualificationFailure::Unknown(detail)) => {
                    assert!(detail.contains("invalid input sysname"), "{detail}")
                }
                _ => panic!("an invalid device identity qualified"),
            }
        }
    }
}
