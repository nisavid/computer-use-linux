//! Minimal X11 protocol client for native X11 sessions.
//!
//! The EWMH window backend lists windows with `wmctrl -lG`, but wmctrl's x/y
//! are not the window origin: it translates the client's offset inside its
//! parent a second time (Red Hat bug 654888, closed WONTFIX). Under a
//! reparenting WM that adds the frame offset twice; under a non-reparenting WM
//! it doubles the absolute position. This module asks the X server directly.
//!
//! It also captures the root window for the native X11 screenshot route:
//! `GetImage` returns device pixels, the same space xdotool/XTEST and these
//! window origins use, with no toolkit scaling layer (issue #155).
//!
//! Callers must gate on [`is_native_x11_session`] first, so XWayland under a
//! Wayland compositor is never used.

use crate::screenshot::{MAX_SCREENSHOT_DECODED_BYTES, MAX_SCREENSHOT_SOURCE_PIXELS};
use anyhow::{anyhow, bail, Context, Result};
use std::{
    env, io,
    net::{IpAddr, SocketAddr, TcpStream},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::net::UnixStream,
    sync::Mutex,
    time::{Duration, Instant},
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, VisualClass, Window,
};
use x11rb::{
    reexports::x11rb_protocol::{
        parse_display::{parse_display, ConnectAddress},
        xauth::get_auth,
    },
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
};

/// Bound on one X11 query, including the connection handshake.
pub(crate) const X11_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
/// Root-window capture moves tens of MB on large screens; allow more time.
pub(crate) const X11_CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// True when this looks like a plain X11 session.
///
/// Requires an X `DISPLAY` and either an explicit `x11` session type or the
/// absence of a Wayland display, so we never hijack XWayland under a Wayland
/// compositor (where a native backend should answer instead).
pub(crate) fn is_native_x11_session() -> bool {
    if env_nonempty("DISPLAY").is_none() {
        return false;
    }
    match env_nonempty("XDG_SESSION_TYPE").as_deref() {
        Some("x11") => true,
        Some("wayland") => false,
        _ => env_nonempty("WAYLAND_DISPLAY").is_none(),
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Run `f` against a fresh X connection off the async runtime, bounded by
/// `limit` so a wedged X server cannot hang a tool call.
pub(crate) async fn with_x11_display<T, F>(limit: Duration, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&X11Display) -> T + Send + 'static,
{
    let deadline = Instant::now() + limit;
    tokio::task::spawn_blocking(move || {
        X11Display::connect_until(deadline).map(|display| f(&display))
    })
    .await
    .map_err(|join_error| anyhow!("X11 query task failed: {join_error}"))?
}

/// The deadline is enforced in the transport itself, including handshake and
/// every reply. Timing out a JoinHandle would leave its blocking task alive.
struct DeadlineStream {
    inner: DefaultStream,
    deadline: Instant,
    incoming: Mutex<IncomingFrame>,
}

// Stage the length prefix before x11rb can allocate its packet buffer. Each
// fresh connection also has an aggregate budget against queued packet floods.
struct IncomingFrame {
    setup: bool,
    header: [u8; 8],
    filled: usize,
    sent: usize,
    body: usize,
    budget: u64,
    max_body: u64,
    failed: bool,
}
impl Default for IncomingFrame {
    fn default() -> Self {
        Self {
            setup: true,
            header: [0; 8],
            filled: 0,
            sent: 0,
            body: 0,
            budget: 1024 * 1024,
            max_body: 1024 * 1024,
            failed: false,
        }
    }
}
fn frame_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "X11 frame exceeds receive limits",
    )
}
fn read_without_fds(stream: &DefaultStream, buf: &mut [u8]) -> io::Result<usize> {
    let mut fds = Vec::new();
    let count = stream.read(buf, &mut fds)?;
    // All requests made by this client use ReplyWithoutFDs. Drop unsolicited
    // descriptors before they can accumulate in x11rb's pending queue.
    if !fds.is_empty() {
        return Err(frame_error());
    }
    Ok(count)
}
impl IncomingFrame {
    fn buffered(&self) -> bool {
        self.failed || (self.filled == 8 && self.sent < 8)
    }
    fn read(&mut self, stream: &DefaultStream, buf: &mut [u8]) -> io::Result<usize> {
        if self.failed {
            return Err(frame_error());
        }
        if buf.is_empty() {
            return Ok(0);
        }
        if self.sent == 8 {
            if self.body > 0 {
                let count = buf.len().min(self.body);
                let n = read_without_fds(stream, &mut buf[..count])?;
                self.body -= n;
                self.budget -= n as u64;
                return Ok(n);
            }
            self.filled = 0;
            self.sent = 0;
        }
        while self.filled < 8 {
            let n = read_without_fds(stream, &mut self.header[self.filled..])?;
            if n == 0 {
                return if self.filled == 0 {
                    Ok(0)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated X11 header",
                    ))
                };
            }
            self.filled += n;
            self.budget = self.budget.checked_sub(n as u64).ok_or_else(frame_error)?;
        }
        if self.sent == 0 {
            // x11rb negotiates the client's native byte order, independently
            // of the image byte order used for pixel conversion.
            let body = if self.setup {
                self.setup = false;
                u64::from(u16::from_ne_bytes(self.header[6..8].try_into().unwrap())) * 4
            } else {
                let extra = if self.header[0] == 1 || self.header[0] & 0x7f == 35 {
                    u64::from(u32::from_ne_bytes(self.header[4..8].try_into().unwrap())) * 4
                } else {
                    0
                };
                extra + 24
            };
            if body > self.max_body || body > self.budget {
                return Err(frame_error());
            }
            self.body = usize::try_from(body).map_err(|_| frame_error())?;
        }
        let n = buf.len().min(8 - self.sent);
        buf[..n].copy_from_slice(&self.header[self.sent..self.sent + n]);
        self.sent += n;
        Ok(n)
    }
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "X server query deadline exceeded"))
}

fn poll_until(fd: RawFd, mode: PollMode, deadline: Instant) -> io::Result<()> {
    let mut pollfd = libc::pollfd {
        fd,
        events: (if mode.readable() { libc::POLLIN } else { 0 })
            | (if mode.writable() { libc::POLLOUT } else { 0 }),
        revents: 0,
    };
    loop {
        let timeout = remaining(deadline)?
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        // SAFETY: pollfd points to one valid descriptor record for this call.
        match unsafe { libc::poll(&mut pollfd, 1, timeout) } {
            0 => {
                remaining(deadline)?;
            }
            -1 => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
            _ => return Ok(()),
        }
    }
}

impl Stream for DeadlineStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        remaining(self.deadline)?;
        if mode.readable() && self.incoming.lock().unwrap().buffered() {
            return Ok(());
        }
        poll_until(self.inner.as_raw_fd(), mode, self.deadline)
    }
    fn read(&self, buf: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        remaining(self.deadline)?;
        let _ = fds;
        let mut incoming = self.incoming.lock().unwrap();
        let result = incoming.read(&self.inner, buf);
        if result
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::InvalidData)
        {
            incoming.failed = true;
        }
        result
    }
    fn write(&self, buf: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        remaining(self.deadline)?;
        self.inner.write(buf, fds)
    }
}

fn connect_unix(path: &str, deadline: Instant) -> io::Result<UnixStream> {
    remaining(deadline)?;
    // SAFETY: socket returns a new descriptor owned below or -1 on failure.
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw is a new owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: zero is valid for sockaddr_un; all used fields are filled below.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as _;
    if path.len() >= address.sun_path.len() || path.as_bytes().contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid X11 Unix socket path",
        ));
    }
    for (dest, &byte) in address.sun_path.iter_mut().zip(path.as_bytes()) {
        *dest = byte as _;
    }
    // SAFETY: address is initialized and the length describes the entire struct.
    let status = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as _,
        )
    };
    if status == -1 {
        let error = io::Error::last_os_error();
        // Unix sockets report EAGAIN when their listen queue is full. Unlike
        // TCP EINPROGRESS this is not an in-flight connection; fail promptly.
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        poll_until(fd.as_raw_fd(), PollMode::Writable, deadline)?;
        let stream = UnixStream::from(fd);
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
        return Ok(stream);
    }
    Ok(UnixStream::from(fd))
}

fn tcp_addresses(host: &str, port: u16, deadline: Instant) -> Result<Vec<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    if host == "localhost" {
        return Ok(vec![
            SocketAddr::from(([127, 0, 0, 1], port)),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
        ]);
    }
    // std's hostname resolver can block indefinitely in NSS. Isolate only
    // that lookup in the existing bounded process runner, never a detached thread.
    let result = crate::command_runner::output_blocking_with_timeout(
        std::process::Command::new("getent").args(["ahosts", "--", host]),
        "resolve X11 host",
        remaining(deadline)?,
    )?;
    let addresses = String::from_utf8_lossy(&result.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next()?.parse::<IpAddr>().ok())
        .map(|ip| SocketAddr::new(ip, port))
        .collect::<Vec<_>>();
    if !result.status.success() || addresses.is_empty() {
        bail!("failed to resolve X11 host {host}");
    }
    Ok(addresses)
}

pub(crate) struct X11Display {
    conn: RustConnection<DeadlineStream>,
    root: Window,
    screen: usize,
}

/// Root-window pixels as tightly packed RGB8.
pub(crate) struct RootImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// `_NET_FRAME_EXTENTS`: decoration widths the WM adds around the client area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameExtents {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl X11Display {
    /// Connect to the display named by `DISPLAY`, authenticating with
    /// `XAUTHORITY` or `~/.Xauthority` the same way Xlib clients do.
    pub(crate) fn connect() -> Result<Self> {
        Self::connect_until(Instant::now() + X11_QUERY_TIMEOUT)
    }

    fn connect_until(deadline: Instant) -> Result<Self> {
        let display = env::var("DISPLAY").context("DISPLAY is not set")?;
        Self::connect_named(&display, deadline)
    }

    fn connect_named(display: &str, deadline: Instant) -> Result<Self> {
        let parsed = parse_display(Some(display))?;
        let screen = usize::from(parsed.screen);
        let mut last_error = anyhow!("no X11 connection addresses");
        let mut connected = None;
        for address in parsed.connect_instruction() {
            let attempt = (|| -> Result<_> {
                match address {
                    ConnectAddress::Socket(path) => Ok(DefaultStream::from_unix_stream(
                        connect_unix(&path, deadline)?,
                    )?),
                    ConnectAddress::Hostname(host, port) => {
                        let mut error = anyhow!("no X11 TCP addresses");
                        for addr in tcp_addresses(host, port, deadline)? {
                            match TcpStream::connect_timeout(&addr, remaining(deadline)?) {
                                Ok(stream) => return Ok(DefaultStream::from_tcp_stream(stream)?),
                                Err(e) => error = e.into(),
                            }
                        }
                        Err(error)
                    }
                    _ => bail!("unsupported X11 address family"),
                }
            })();
            match attempt {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(error) => last_error = error,
            }
        }
        let (inner, (family, address)) = connected
            .ok_or(last_error)
            .context("failed to connect to the X server named by DISPLAY")?;
        let (auth_name, auth_data) = get_auth(family, &address, parsed.display)
            .unwrap_or(None)
            .unwrap_or_default();
        let conn = RustConnection::connect_to_stream_with_auth_info(
            DeadlineStream {
                inner,
                deadline,
                incoming: Mutex::new(IncomingFrame::default()),
            },
            screen,
            auth_name,
            auth_data,
        )
        .context("failed X11 connection handshake")?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .context("X server reported no screen for DISPLAY")?
            .root;
        Ok(Self { conn, root, screen })
    }

    /// `DISPLAY` screen size and root depth, for doctor.
    pub(crate) fn describe(&self) -> String {
        let screen = &self.conn.setup().roots[self.screen];
        format!(
            "native X11 root window {}x{}, depth {}",
            screen.width_in_pixels, screen.height_in_pixels, screen.root_depth
        )
    }

    /// Capture the whole root window with one `GetImage` request.
    pub(crate) fn capture_root(&self) -> Result<RootImage> {
        let setup = self.conn.setup();
        let screen = &setup.roots[self.screen];
        let (width, height) = (screen.width_in_pixels, screen.height_in_pixels);
        let format = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == screen.root_depth)
            .context("no pixmap format for root depth")?;
        let (_, native, _) = image_sizes(
            u32::from(width),
            u32::from(height),
            u32::from(format.bits_per_pixel),
            u32::from(format.scanline_pad),
        )?;
        {
            let mut incoming = self.conn.stream().incoming.lock().unwrap();
            let wire = native.div_ceil(4) as u64 * 4;
            incoming.max_body = wire + 24;
            incoming.budget = wire + 1024 * 1024;
        }
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, 0, 0, width, height, !0)?
            .reply()
            .context("X server refused GetImage on the root window")?;
        if reply.depth != screen.root_depth {
            bail!("GetImage depth differs from root depth");
        }
        let visual = screen
            .allowed_depths
            .iter()
            .flat_map(|depth| &depth.visuals)
            .find(|visual| visual.visual_id == reply.visual)
            .with_context(|| format!("root image visual 0x{:x} is not listed", reply.visual))?;
        if !matches!(
            visual.class,
            VisualClass::TRUE_COLOR | VisualClass::DIRECT_COLOR
        ) {
            bail!(
                "root visual class {:?} is not TrueColor or DirectColor; palette-based displays are not supported",
                visual.class
            );
        }
        let format = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == reply.depth)
            .with_context(|| format!("no pixmap format for depth {}", reply.depth))?;
        let layout = ZPixmapLayout {
            bits_per_pixel: u32::from(format.bits_per_pixel),
            scanline_pad: u32::from(format.scanline_pad),
            msb_first: setup.image_byte_order == ImageOrder::MSB_FIRST,
            red_mask: visual.red_mask,
            green_mask: visual.green_mask,
            blue_mask: visual.blue_mask,
        };
        let rgb = zpixmap_to_rgb(&reply.data, u32::from(width), u32::from(height), &layout)?;
        Ok(RootImage {
            width: u32::from(width),
            height: u32::from(height),
            rgb,
        })
    }

    /// Absolute root-window origin of each window's client area, the value
    /// `xwininfo` prints as "Absolute upper-left". Requests are pipelined, so
    /// this costs one round trip for the whole list. `None` marks a window that
    /// vanished, is invalid, or sits on another screen.
    pub(crate) fn client_origins(&self, windows: &[Window]) -> Vec<Option<(i32, i32)>> {
        let cookies = windows
            .iter()
            .map(|&window| {
                self.conn
                    .translate_coordinates(window, self.root, 0, 0)
                    .ok()
            })
            .collect::<Vec<_>>();
        cookies
            .into_iter()
            .map(|cookie| {
                let reply = cookie?.reply().ok()?;
                reply
                    .same_screen
                    .then(|| (i32::from(reply.dst_x), i32::from(reply.dst_y)))
            })
            .collect()
    }

    /// The WM's `_NET_FRAME_EXTENTS` for `window`, or `None` when the WM does
    /// not publish it.
    pub(crate) fn frame_extents(&self, window: Window) -> Result<Option<FrameExtents>> {
        let atom = self
            .conn
            .intern_atom(true, b"_NET_FRAME_EXTENTS")?
            .reply()
            .context("failed to intern _NET_FRAME_EXTENTS")?
            .atom;
        if atom == x11rb::NONE {
            return Ok(None);
        }
        let reply = self
            .conn
            .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 4)?
            .reply()
            .context("failed to read _NET_FRAME_EXTENTS")?;
        Ok(parse_frame_extents(
            reply.value32().map(|values| values.collect::<Vec<_>>()),
        ))
    }
}

fn parse_frame_extents(values: Option<Vec<u32>>) -> Option<FrameExtents> {
    match values.as_deref() {
        Some(&[left, right, top, bottom]) => Some(FrameExtents {
            left,
            right,
            top,
            bottom,
        }),
        _ => None,
    }
}

/// Frame (outer) origin from a client origin and the WM's extents. With the
/// default NorthWest gravity this is the point `wmctrl -e 0,x,y,...` places.
pub(crate) fn frame_origin(client_origin: (i32, i32), extents: FrameExtents) -> (i32, i32) {
    (
        client_origin
            .0
            .saturating_sub(i32::try_from(extents.left).unwrap_or(i32::MAX)),
        client_origin
            .1
            .saturating_sub(i32::try_from(extents.top).unwrap_or(i32::MAX)),
    )
}

/// How the server packs one ZPixmap scanline, from the connection setup and
/// the image's visual.
#[derive(Clone, Copy, Debug)]
struct ZPixmapLayout {
    bits_per_pixel: u32,
    scanline_pad: u32,
    msb_first: bool,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
}

// Compute native, padded, and RGB sizes before requesting or allocating pixels.
fn image_sizes(width: u32, height: u32, bpp: u32, pad: u32) -> Result<(usize, usize, usize)> {
    if width == 0 || height == 0 {
        bail!("X11 root image has zero dimensions");
    }
    if !matches!(bpp, 16 | 24 | 32) || !matches!(pad, 8 | 16 | 32) {
        bail!("unsupported X11 pixel layout");
    }
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .context("X11 pixel overflow")?;
    if pixels > MAX_SCREENSHOT_SOURCE_PIXELS {
        bail!("X11 pixel source limit exceeded");
    }
    let row = u64::from(width)
        .checked_mul(u64::from(bpp))
        .context("X11 row overflow")?;
    let stride = row
        .div_ceil(u64::from(pad))
        .checked_mul(u64::from(pad) / 8)
        .context("X11 stride overflow")?;
    let native = stride
        .checked_mul(u64::from(height))
        .context("X11 native size overflow")?;
    let rgb = pixels.checked_mul(3).context("X11 RGB size overflow")?;
    if native > MAX_SCREENSHOT_DECODED_BYTES || rgb > MAX_SCREENSHOT_DECODED_BYTES {
        bail!("X11 decoded-byte limit exceeded");
    }
    Ok((
        usize::try_from(stride)?,
        usize::try_from(native)?,
        usize::try_from(rgb)?,
    ))
}

fn zpixmap_to_rgb(data: &[u8], width: u32, height: u32, layout: &ZPixmapLayout) -> Result<Vec<u8>> {
    let (stride, needed, rgb_bytes) =
        image_sizes(width, height, layout.bits_per_pixel, layout.scanline_pad)?;
    let bytes_per_pixel = match layout.bits_per_pixel {
        16 | 24 | 32 => (layout.bits_per_pixel / 8) as usize,
        other => bail!("{other}-bit ZPixmap pixels are not supported"),
    };
    let (width, height) = (width as usize, height as usize);
    if data.len() < needed || data.len() > needed.div_ceil(4) * 4 {
        bail!(
            "GetImage returned {} bytes; {width}x{height} at stride {stride} needs {needed}",
            data.len()
        );
    }
    let mut rgb = Vec::new();
    rgb.try_reserve_exact(rgb_bytes)
        .context("failed to allocate X11 RGB image")?;
    for row in data[..needed].chunks_exact(stride) {
        for pixel in row[..width * bytes_per_pixel].chunks_exact(bytes_per_pixel) {
            let value = pixel.iter().enumerate().fold(0_u32, |acc, (index, &byte)| {
                let shift = if layout.msb_first {
                    8 * (bytes_per_pixel - 1 - index)
                } else {
                    8 * index
                };
                acc | (u32::from(byte) << shift)
            });
            rgb.push(channel_to_u8(value, layout.red_mask));
            rgb.push(channel_to_u8(value, layout.green_mask));
            rgb.push(channel_to_u8(value, layout.blue_mask));
        }
    }
    Ok(rgb)
}

/// Extract the channel selected by `mask` and scale it to 8 bits.
fn channel_to_u8(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let value = (pixel & mask) >> shift;
    if bits >= 8 {
        (value >> (bits - 8)) as u8
    } else {
        let max = (1_u32 << bits) - 1;
        ((value * 255 + max / 2) / max) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_capture(width: u16, height: u16, oversized: bool) -> (Result<RootImage>, Vec<u8>) {
        use std::io::{Read, Write};
        use x11rb::protocol::xproto::{Depth, Format, Screen, Setup, Visualtype};
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let mut seq = 0_u16;
            loop {
                let mut header = [0; 4];
                if server.read_exact(&mut header).is_err() {
                    break;
                }
                let len = usize::from(u16::from_ne_bytes([header[2], header[3]])) * 4;
                let mut rest = vec![0; len - 4];
                server.read_exact(&mut rest).unwrap();
                requests.push(header[0]);
                seq += 1;
                let mut reply = [0; 32];
                reply[0] = 1;
                reply[2..4].copy_from_slice(&seq.to_ne_bytes());
                match header[0] {
                    98 => server.write_all(&reply).unwrap(),
                    73 => {
                        reply[1] = 24;
                        reply[4..8]
                            .copy_from_slice(&(if oversized { u32::MAX } else { 1 }).to_ne_bytes());
                        reply[8..12].copy_from_slice(&2_u32.to_ne_bytes());
                        if oversized {
                            server.write_all(&reply[..8]).unwrap();
                        } else {
                            server.write_all(&reply).unwrap();
                            server.write_all(&[0x56, 0x34, 0x12, 0]).unwrap();
                        }
                    }
                    other => panic!("unexpected opcode {other}"),
                }
            }
            requests
        });
        let setup = Setup {
            resource_id_base: 0x02000000,
            resource_id_mask: 0x001fffff,
            maximum_request_length: u16::MAX,
            image_byte_order: ImageOrder::LSB_FIRST,
            pixmap_formats: vec![Format {
                depth: 24,
                bits_per_pixel: 32,
                scanline_pad: 32,
            }],
            roots: vec![Screen {
                root: 1,
                root_depth: 24,
                root_visual: 2,
                width_in_pixels: width,
                height_in_pixels: height,
                allowed_depths: vec![Depth {
                    depth: 24,
                    visuals: vec![Visualtype {
                        visual_id: 2,
                        class: VisualClass::TRUE_COLOR,
                        red_mask: 0xff0000,
                        green_mask: 0xff00,
                        blue_mask: 0xff,
                        ..Visualtype::default()
                    }],
                }],
                ..Screen::default()
            }],
            ..Setup::default()
        };
        let stream = test_stream(
            client,
            IncomingFrame {
                setup: false,
                ..IncomingFrame::default()
            },
        );
        let display = X11Display {
            conn: RustConnection::for_connected_stream(stream, setup).unwrap(),
            root: 1,
            screen: 0,
        };
        let result = display.capture_root();
        drop(display);
        (result, worker.join().unwrap())
    }
    fn test_stream(socket: UnixStream, incoming: IncomingFrame) -> DeadlineStream {
        DeadlineStream {
            inner: DefaultStream::from_unix_stream(socket).unwrap().0,
            deadline: Instant::now() + Duration::from_secs(2),
            incoming: Mutex::new(incoming),
        }
    }
    #[test]
    fn capture_limits_precede_request_and_reply_allocation() {
        let (result, requests) = fixture_capture(u16::MAX, u16::MAX, false);
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("pixel source limit"));
        assert!(requests.is_empty());
        let (result, requests) = fixture_capture(1, 1, true);
        assert!(format!("{:#}", result.err().unwrap()).contains("receive limits"));
        assert!(requests.contains(&73));
        let image = fixture_capture(1, 1, false).0.unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.rgb, [0x12, 0x34, 0x56]);
    }
    #[test]
    fn layout_sizes_are_checked_without_allocating() {
        assert!(image_sizes(7680, 4320, 32, 32).is_ok());
        assert_eq!(
            image_sizes(16384, 8192, 32, 32).unwrap().1 as u64,
            MAX_SCREENSHOT_DECODED_BYTES
        );
        for (w, h) in [(0, 1), (1, 0), (u32::MAX, u32::MAX), (16384, 8193)] {
            assert!(image_sizes(w, h, 32, 32).is_err());
        }
        for pad in [0, 1, 9, 255, u32::MAX] {
            assert!(image_sizes(1, 1, 16, pad).is_err());
        }
        assert!(zpixmap_to_rgb(&[0; 8], 1, 1, &BGRX).is_err());
    }
    #[test]
    fn frame_guard_handles_fragmentation_floods_and_generic_events() {
        use std::io::Write;
        let (client, mut server) = UnixStream::pair().unwrap();
        let stream = test_stream(
            client,
            IncomingFrame {
                setup: false,
                ..IncomingFrame::default()
            },
        );
        let mut header = [0; 8];
        header[0] = 0xa3;
        header[4..8].copy_from_slice(&u32::MAX.to_ne_bytes());
        let mut out = [0; 32];
        let mut fds = Vec::new();
        server.write_all(&header[..3]).unwrap();
        assert_eq!(
            stream.read(&mut out, &mut fds).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        server.write_all(&header[3..]).unwrap();
        assert_eq!(
            stream.read(&mut out, &mut fds).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            stream.read(&mut out, &mut fds).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let (client, mut server) = UnixStream::pair().unwrap();
        let stream = test_stream(
            client,
            IncomingFrame {
                setup: false,
                budget: 63,
                ..IncomingFrame::default()
            },
        );
        let mut packets = [0; 64];
        packets[0] = 1;
        packets[32] = 1;
        server.write_all(&packets).unwrap();
        assert_eq!(stream.read(&mut out, &mut fds).unwrap(), 8);
        assert_eq!(stream.read(&mut out, &mut fds).unwrap(), 24);
        assert!(stream.read(&mut out, &mut fds).is_err());
    }
    #[test]
    fn descriptor_guard_covers_headers_and_bodies() {
        use rustix::net::{sendmsg, SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
        use std::{io::Write, os::fd::AsFd};
        for body in [false, true] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let stream = test_stream(
                client,
                IncomingFrame {
                    setup: false,
                    ..IncomingFrame::default()
                },
            );
            let mut out = [0; 32];
            let mut received = Vec::new();
            let mut header = [0; 8];
            header[0] = 1;
            if body {
                server.write_all(&header).unwrap();
                assert_eq!(stream.read(&mut out, &mut received).unwrap(), 8);
            }
            let file = std::fs::File::open("/dev/null").unwrap();
            let fds = [file.as_fd()];
            let mut storage = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut ancillary = SendAncillaryBuffer::new(&mut storage);
            assert!(ancillary.push(SendAncillaryMessage::ScmRights(&fds)));
            let tail = [0; 24];
            let bytes = if body { &tail[..] } else { &header[..] };
            sendmsg(
                &server,
                &[io::IoSlice::new(bytes)],
                &mut ancillary,
                SendFlags::NOSIGNAL,
            )
            .unwrap();
            assert_eq!(
                stream.read(&mut out, &mut received).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert!(received.is_empty());
        }
    }

    #[test]
    fn stalled_servers_release_queries_and_runtime() {
        const CHILD: &str = "COMPUTER_USE_LINUX_TEST_X11_DEADLINE_CHILD";
        if env::var_os(CHILD).is_some() {
            use std::io::{Read, Write};
            use std::net::TcpListener;
            use x11rb::protocol::xproto::{Format, Screen, Setup};
            use x11rb::reexports::x11rb_protocol::x11_utils::Serialize;

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            assert!(port >= 6000);
            env::set_var("DISPLAY", format!("127.0.0.1:{}", port - 6000));
            env::set_var("XAUTHORITY", "/dev/null");
            let server = std::thread::spawn(move || {
                for index in 0..6 {
                    let (mut peer, _) = listener.accept().unwrap();
                    let mut header = [0; 12];
                    peer.read_exact(&mut header).unwrap();
                    let name = u16::from_ne_bytes([header[6], header[7]]) as usize;
                    let data = u16::from_ne_bytes([header[8], header[9]]) as usize;
                    let mut auth = vec![0; name.div_ceil(4) * 4 + data.div_ceil(4) * 4];
                    peer.read_exact(&mut auth).unwrap();
                    if index >= 3 {
                        let mut setup = Setup {
                            status: 1,
                            protocol_major_version: 11,
                            resource_id_base: 0x02000000,
                            resource_id_mask: 0x001fffff,
                            maximum_request_length: u16::MAX,
                            pixmap_formats: vec![Format {
                                depth: 24,
                                bits_per_pixel: 32,
                                scanline_pad: 32,
                            }],
                            roots: vec![Screen {
                                root: 1,
                                width_in_pixels: 16,
                                height_in_pixels: 16,
                                root_depth: 24,
                                ..Screen::default()
                            }],
                            ..Setup::default()
                        };
                        setup.length = ((setup.serialize().len() - 8) / 4) as u16;
                        peer.write_all(&setup.serialize()).unwrap();
                    }
                    // Deliberately never answer handshake/reply. EOF proves
                    // the timed-out query actually dropped its transport.
                    let mut requests = Vec::new();
                    peer.read_to_end(&mut requests).unwrap();
                    if index >= 3 {
                        assert!(!requests.is_empty());
                    }
                }
            });
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let started = Instant::now();
            runtime.block_on(async {
                for _ in 0..3 {
                    assert!(
                        with_x11_display(Duration::from_millis(70), |display| display.describe())
                            .await
                            .is_err()
                    );
                }
                // All reply consumers share the deadline, including geometry,
                // screenshot and the pipelined per-window origin queries.
                assert!(
                    with_x11_display(Duration::from_millis(70), |display| display
                        .frame_extents(1))
                    .await
                    .unwrap()
                    .is_err()
                );
                assert!(
                    with_x11_display(Duration::from_millis(70), |display| display.capture_root())
                        .await
                        .unwrap()
                        .is_err()
                );
                assert_eq!(
                    with_x11_display(Duration::from_millis(70), |display| display
                        .client_origins(&[1, 2]))
                    .await
                    .unwrap(),
                    vec![None, None]
                );
            });
            // Unlike timeout(JoinHandle), the runtime has no blocked workers
            // left to wait for, even while the fake server stays connected.
            drop(runtime);
            server.join().unwrap();
            assert!(started.elapsed() < Duration::from_secs(2));
            return;
        }
        // A subprocess makes runtime-exit regression fail in bounded time,
        // rather than hanging the entire test runner on drop(Runtime).
        let mut child = std::process::Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "x11_display::tests::stalled_servers_release_queries_and_runtime",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("X11 queries or runtime failed to terminate after transport deadlines");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn unix_connect_fails_promptly_when_listen_queue_is_full() {
        use std::os::unix::net::UnixListener;
        let path = format!(
            "/tmp/computer-use-linux-x11-deadline-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
        );
        let listener = UnixListener::bind(&path).unwrap();
        // SAFETY: listener owns a valid listening socket; shrink its backlog.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let queued = UnixStream::connect(&path).unwrap();
        let started = Instant::now();
        assert!(connect_unix(&path, started + Duration::from_millis(70)).is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        drop(queued);
        drop(listener);
        std::fs::remove_file(path).unwrap();
    }

    const BGRX: ZPixmapLayout = ZPixmapLayout {
        bits_per_pixel: 32,
        scanline_pad: 32,
        msb_first: false,
        red_mask: 0x00ff_0000,
        green_mask: 0x0000_ff00,
        blue_mask: 0x0000_00ff,
    };

    #[test]
    fn depth24_lsb_first_is_bgrx_bytes() {
        // Two pixels: pure red, then 0x123456. Bytes are B,G,R,X per pixel.
        let data = [0x00, 0x00, 0xff, 0x00, 0x56, 0x34, 0x12, 0xaa];
        assert_eq!(
            zpixmap_to_rgb(&data, 2, 1, &BGRX).unwrap(),
            vec![0xff, 0x00, 0x00, 0x12, 0x34, 0x56]
        );
    }

    #[test]
    fn msb_first_reads_bytes_big_endian() {
        let layout = ZPixmapLayout {
            msb_first: true,
            ..BGRX
        };
        let data = [0x00, 0x12, 0x34, 0x56];
        assert_eq!(
            zpixmap_to_rgb(&data, 1, 1, &layout).unwrap(),
            vec![0x12, 0x34, 0x56]
        );
    }

    #[test]
    fn rgb565_scales_five_and_six_bit_channels_to_full_range() {
        let layout = ZPixmapLayout {
            bits_per_pixel: 16,
            scanline_pad: 32,
            msb_first: false,
            red_mask: 0xf800,
            green_mask: 0x07e0,
            blue_mask: 0x001f,
        };
        // white 0xffff, black 0x0000; width 2 at 16 bpp is already 32-bit aligned.
        let data = [0xff, 0xff, 0x00, 0x00];
        assert_eq!(
            zpixmap_to_rgb(&data, 2, 1, &layout).unwrap(),
            vec![255, 255, 255, 0, 0, 0]
        );
    }

    #[test]
    fn scanline_padding_is_skipped_between_rows() {
        let layout = ZPixmapLayout {
            bits_per_pixel: 16,
            scanline_pad: 32,
            msb_first: false,
            red_mask: 0xf800,
            green_mask: 0x07e0,
            blue_mask: 0x001f,
        };
        // Width 1 at 16 bpp pads each row to 4 bytes; the pad bytes are junk.
        let data = [0xff, 0xff, 0xee, 0xee, 0x00, 0x00, 0xee, 0xee];
        assert_eq!(
            zpixmap_to_rgb(&data, 1, 2, &layout).unwrap(),
            vec![255, 255, 255, 0, 0, 0]
        );
    }

    #[test]
    fn ten_bit_channels_keep_their_top_eight_bits() {
        let layout = ZPixmapLayout {
            red_mask: 0x3ff0_0000,
            green_mask: 0x000f_fc00,
            blue_mask: 0x0000_03ff,
            ..BGRX
        };
        let pixel: u32 = (0x3ff << 20) | (0x200 << 10) | 0x001;
        assert_eq!(
            zpixmap_to_rgb(&pixel.to_le_bytes(), 1, 1, &layout).unwrap(),
            vec![0xff, 0x80, 0x00]
        );
    }

    #[test]
    fn short_or_unsupported_images_are_errors() {
        assert!(zpixmap_to_rgb(&[0; 7], 2, 1, &BGRX).is_err());
        let one_bit = ZPixmapLayout {
            bits_per_pixel: 1,
            ..BGRX
        };
        assert!(zpixmap_to_rgb(&[0; 4], 1, 1, &one_bit).is_err());
    }

    #[test]
    fn frame_extents_need_exactly_four_cardinals() {
        assert_eq!(
            parse_frame_extents(Some(vec![1, 1, 22, 5])),
            Some(FrameExtents {
                left: 1,
                right: 1,
                top: 22,
                bottom: 5
            })
        );
        assert_eq!(parse_frame_extents(None), None);
        assert_eq!(parse_frame_extents(Some(vec![1, 1, 22])), None);
        assert_eq!(parse_frame_extents(Some(vec![])), None);
    }

    #[test]
    fn frame_origin_subtracts_left_and_top_extents() {
        // openbox measurement: client at 301,222 with extents 1,1,22,5 was
        // placed by `wmctrl -e 0,300,200,...`.
        let extents = FrameExtents {
            left: 1,
            right: 1,
            top: 22,
            bottom: 5,
        };
        assert_eq!(frame_origin((301, 222), extents), (300, 200));
        assert_eq!(
            frame_origin((0, 0), FrameExtents::default()),
            (0, 0),
            "an undecorated window's frame is its client area"
        );
    }
}
