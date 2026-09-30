use super::Stroke;
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs, io, mem,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const OP_STROKE: u16 = 1;
const OP_FINISH: u16 = 2;
const OP_CANCEL: u16 = 3;
const OP_REVOKED: u16 = 0xffff;
const WAIT: Duration = Duration::from_secs(2);
const MAX_STROKES: u32 = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Identity {
    pub(super) instance: [u8; 16],
    pub(super) input_sysname: String,
    raw_device: u64,
    raw_inode: u64,
}

pub(super) struct Channel {
    endpoint: OwnedFd,
    pub(super) identity: Identity,
    raw_socket: PathBuf,
    next_sequence: u32,
    opened_at: Instant,
    last_used: Instant,
    closed: bool,
    pending: Option<(u16, u32)>,
    pub(super) may_have_submitted: bool,
}

impl Channel {
    pub(super) fn open(identity_socket: &Path, raw_socket: &Path) -> Result<Self> {
        let raw =
            fs::symlink_metadata(raw_socket).context("configured raw socket is unavailable")?;
        ensure!(
            raw.file_type().is_socket(),
            "configured raw socket is not a socket pathname"
        );
        let deadline = Instant::now() + WAIT;
        let control = connect(identity_socket, deadline)?;
        let producer = credentials(control.as_raw_fd())?;
        same_pid_namespace(producer.pid)?;
        let mut request = [0; 16];
        request[..8].copy_from_slice(b"YDOTID1\0");
        request[8..10].copy_from_slice(&1_u16.to_le_bytes());
        request[10..12].copy_from_slice(&1_u16.to_le_bytes());
        request[12..16].copy_from_slice(&16_u32.to_le_bytes());
        send_record(control.as_raw_fd(), &request, deadline)?;
        let (response, mut descriptors) = receive_record(control.as_raw_fd(), 96, deadline)?;
        ensure!(response.len() == 96, "invalid identity response length");
        ensure!(
            &response[..8] == b"YDOTID1\0"
                && u16_at(&response, 8) == 1
                && u32_at(&response, 12) == 96,
            "invalid identity response header"
        );
        let status = u16_at(&response, 10);
        if status != 0 {
            ensure!(
                status <= 3
                    && descriptors.is_empty()
                    && response[16..].iter().all(|byte| *byte == 0),
                "invalid identity error response"
            );
            bail!("verified producer refused open (status {status})");
        }
        ensure!(
            descriptors.len() == 1,
            "identity success must transfer exactly one channel"
        );
        ensure!(
            u32_at(&response, 80) == MAX_STROKES
                && u32_at(&response, 84) == 1
                && response[88..].iter().all(|byte| *byte == 0),
            "unsupported identity limits or features"
        );
        let name_bytes = &response[32..64];
        let nul = name_bytes
            .iter()
            .position(|byte| *byte == 0)
            .context("identity sysname lacks NUL padding")?;
        ensure!(
            name_bytes[nul..].iter().all(|byte| *byte == 0),
            "invalid identity sysname padding"
        );
        let input_sysname = std::str::from_utf8(&name_bytes[..nul])
            .context("invalid identity sysname")?
            .to_owned();
        ensure!(
            input_sysname.starts_with("input")
                && input_sysname.len() > 5
                && input_sysname[5..].bytes().all(|byte| byte.is_ascii_digit()),
            "invalid uinput sysname"
        );
        let raw_device = u64_at(&response, 64);
        let raw_inode = u64_at(&response, 72);
        ensure!(
            raw_device == raw.dev() && raw_inode == raw.ino(),
            "identity does not describe the configured raw socket"
        );
        let endpoint = descriptors
            .pop()
            .expect("exactly one descriptor checked above");
        ensure!(
            socket_type(endpoint.as_raw_fd())? == libc::SOCK_SEQPACKET,
            "transferred endpoint is not SEQPACKET"
        );
        let peer = credentials(endpoint.as_raw_fd())?;
        ensure!(
            peer.pid == producer.pid && peer.uid == producer.uid && peer.gid == producer.gid,
            "transferred endpoint has another producer"
        );
        same_pid_namespace(peer.pid)?;
        set_nonblocking(endpoint.as_raw_fd())?;
        let now = Instant::now();
        let mut instance = [0; 16];
        instance.copy_from_slice(&response[16..32]);
        let channel = Self {
            endpoint,
            identity: Identity {
                instance,
                input_sysname,
                raw_device,
                raw_inode,
            },
            raw_socket: raw_socket.to_owned(),
            next_sequence: 1,
            opened_at: now,
            last_used: now,
            closed: false,
            pending: None,
            may_have_submitted: false,
        };
        channel.recheck_socket()?;
        Ok(channel)
    }

    pub(super) fn recheck_socket(&self) -> Result<()> {
        let metadata =
            fs::symlink_metadata(&self.raw_socket).context("configured raw socket disappeared")?;
        ensure!(
            metadata.file_type().is_socket()
                && metadata.dev() == self.identity.raw_device
                && metadata.ino() == self.identity.raw_inode,
            "configured raw socket was replaced"
        );
        Ok(())
    }

    pub(super) fn stroke(
        &mut self,
        stroke: Stroke,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        ensure!(
            valid_keycode(stroke.keycode),
            "unsupported printable keycode"
        );
        ensure!(
            self.next_sequence <= MAX_STROKES,
            "verified stroke limit exceeded"
        );
        self.exchange(
            OP_STROKE,
            u16::from(stroke.left_shift),
            stroke.keycode,
            cancelled,
        )
    }

    pub(super) fn finish(&mut self, cancelled: &mut dyn FnMut() -> bool) -> Result<()> {
        self.exchange(OP_FINISH, 0, 0, cancelled)?;
        self.closed = true;
        Ok(())
    }

    pub(super) fn cancel(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let result = self.exchange_record(OP_CANCEL, 0, 0, Instant::now() + WAIT, &mut || false);
        self.closed = true;
        unsafe {
            libc::shutdown(self.endpoint.as_raw_fd(), libc::SHUT_RDWR);
        }
        result
    }

    fn exchange(
        &mut self,
        operation: u16,
        flags: u16,
        keycode: u16,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        ensure!(!self.closed, "verified channel is closed");
        ensure!(!cancelled(), "verified typing cancelled before submission");
        ensure!(
            self.opened_at.elapsed() < Duration::from_secs(300)
                && self.last_used.elapsed() < Duration::from_secs(10),
            "verified channel deadline expired"
        );
        self.recheck_socket()?;
        self.exchange_record(operation, flags, keycode, Instant::now() + WAIT, cancelled)
    }

    fn exchange_record(
        &mut self,
        operation: u16,
        flags: u16,
        keycode: u16,
        deadline: Instant,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<()> {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let mut request = [0; 16];
        request[..4].copy_from_slice(b"YDP1");
        request[4..6].copy_from_slice(&operation.to_le_bytes());
        request[6..8].copy_from_slice(&flags.to_le_bytes());
        request[8..12].copy_from_slice(&sequence.to_le_bytes());
        request[12..14].copy_from_slice(&keycode.to_le_bytes());
        // Treat even a failed attempted stroke send as possibly submitted. Never replay it.
        let previous_pending = self.pending;
        self.pending = Some((operation, sequence));
        if operation == OP_STROKE {
            self.may_have_submitted = true;
        }
        send_record(self.endpoint.as_raw_fd(), &request, deadline)?;
        let mut drained_previous = false;
        loop {
            ensure!(
                !cancelled(),
                "verified typing cancelled while awaiting acknowledgement"
            );
            wait_fd(
                self.endpoint.as_raw_fd(),
                libc::POLLIN,
                deadline.min(Instant::now() + Duration::from_millis(10)),
            )
            .or_else(|error| {
                if error.kind() == io::ErrorKind::TimedOut && Instant::now() < deadline {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
            let received = receive_now(self.endpoint.as_raw_fd(), 16);
            let (reply, descriptors) = match received {
                Err(error)
                    if error
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::WouldBlock) =>
                {
                    continue
                }
                result => result?,
            };
            ensure!(
                reply.len() == 16 && descriptors.is_empty() && &reply[..4] == b"YDP1",
                "invalid verified acknowledgement"
            );
            let reply_operation = u16_at(&reply, 4);
            let status = u16_at(&reply, 6);
            let error = u32_at(&reply, 12);
            ensure!(
                (status == 0 && error == 0)
                    || (status == 1 && (1..=5).contains(&error))
                    || (status == 2 && (6..=8).contains(&error))
                    || (status == 3 && error == 9),
                "invalid verified acknowledgement status"
            );
            if reply_operation == OP_REVOKED {
                ensure!(
                    status == 2 || status == 3,
                    "invalid asynchronous revocation"
                );
                bail!("verified channel revoked (status {status}, error {error})");
            }
            if operation == OP_CANCEL
                && !drained_previous
                && previous_pending.is_some_and(|(old_operation, old_sequence)| {
                    reply_operation == old_operation | 0x8000 && u32_at(&reply, 8) == old_sequence
                })
            {
                drained_previous = true;
                continue;
            }
            ensure!(
                reply_operation == operation | 0x8000 && u32_at(&reply, 8) == sequence,
                "verified acknowledgement does not match the request"
            );
            ensure!(
                status == 0,
                "verified producer failed request (status {status}, error {error})"
            );
            self.last_used = Instant::now();
            self.pending = None;
            return Ok(());
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

pub(super) fn valid_keycode(code: u16) -> bool {
    matches!(code, 2..=13 | 16..=27 | 30..=41 | 43..=53 | 57)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        bytes[offset..offset + 2]
            .try_into()
            .expect("validated fixed length"),
    )
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated fixed length"),
    )
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("validated fixed length"),
    )
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn socket_type(fd: RawFd) -> io::Result<i32> {
    let mut value = 0_i32;
    let mut length = mem::size_of_val(&value) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut value as *mut i32).cast(),
            &mut length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn credentials(fd: RawFd) -> io::Result<libc::ucred> {
    let mut value: libc::ucred = unsafe { mem::zeroed() };
    let mut length = mem::size_of_val(&value) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut value as *mut libc::ucred).cast(),
            &mut length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if length as usize != mem::size_of_val(&value) || value.pid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid producer credentials",
        ));
    }
    Ok(value)
}

fn same_pid_namespace(pid: libc::pid_t) -> Result<()> {
    let current = fs::metadata("/proc/self/ns/pid")?;
    let producer = fs::metadata(format!("/proc/{pid}/ns/pid"))?;
    ensure!(
        current.dev() == producer.dev() && current.ino() == producer.ino(),
        "producer is in another PID namespace"
    );
    Ok(())
}

fn connect(path: &Path, deadline: Instant) -> Result<OwnedFd> {
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    ensure!(
        !bytes.is_empty() && bytes.len() < address.sun_path.len() && !bytes.contains(&0),
        "invalid identity socket pathname"
    );
    address.sun_family = libc::AF_UNIX as _;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes) {
        *destination = *source as _;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            mem::size_of_val(&address) as _,
        )
    } < 0
    {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error).context("verified identity socket connection failed");
        }
        wait_fd(fd.as_raw_fd(), libc::POLLOUT, deadline)?;
        let mut error = 0_i32;
        let mut length = mem::size_of_val(&error) as _;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut i32).cast(),
                &mut length,
            )
        } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error).into());
        }
    }
    Ok(fd)
}

pub(super) fn wait_fd(fd: RawFd, events: i16, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "verified I/O deadline expired")
            })?;
        let mut poll = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let milliseconds = remaining.as_millis().max(1).min(i32::MAX as u128) as i32;
        let result = unsafe { libc::poll(&mut poll, 1, milliseconds) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if result == 0 {
            continue;
        }
        if poll.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid verified channel descriptor",
            ));
        }
        return Ok(());
    }
}

fn send_record(fd: RawFd, bytes: &[u8], deadline: Instant) -> Result<()> {
    loop {
        let length = unsafe {
            libc::send(
                fd,
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if length == bytes.len() as isize {
            return Ok(());
        }
        if length >= 0 {
            bail!("short verified record write");
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        wait_fd(fd, libc::POLLOUT, deadline)?;
    }
}

fn receive_record(fd: RawFd, size: usize, deadline: Instant) -> Result<(Vec<u8>, Vec<OwnedFd>)> {
    loop {
        wait_fd(fd, libc::POLLIN, deadline)?;
        match receive_now(fd, size) {
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::WouldBlock) =>
            {
                continue
            }
            result => return result,
        }
    }
}

fn receive_now(fd: RawFd, size: usize) -> Result<(Vec<u8>, Vec<OwnedFd>)> {
    let mut bytes = vec![0; size];
    let mut control = [0_usize; 64];
    let mut iov = libc::iovec {
        iov_base: bytes.as_mut_ptr().cast(),
        iov_len: bytes.len(),
    };
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = mem::size_of_val(&control);
    let length = unsafe {
        libc::recvmsg(
            fd,
            &mut message,
            libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC | libc::MSG_TRUNC,
        )
    };
    if length < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut descriptors = Vec::new();
    let mut ancillary_valid = true;
    let mut header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    while !header.is_null() {
        let value = unsafe { &*header };
        let minimum = unsafe { libc::CMSG_LEN(0) } as usize;
        if value.cmsg_len < minimum {
            ancillary_valid = false;
            break;
        }
        if value.cmsg_level != libc::SOL_SOCKET || value.cmsg_type != libc::SCM_RIGHTS {
            ancillary_valid = false;
        } else {
            let count = (value.cmsg_len - minimum) / mem::size_of::<RawFd>();
            ancillary_valid &= (value.cmsg_len - minimum).is_multiple_of(mem::size_of::<RawFd>());
            for index in 0..count {
                let received = unsafe {
                    libc::CMSG_DATA(header)
                        .cast::<RawFd>()
                        .add(index)
                        .read_unaligned()
                };
                descriptors.push(unsafe { OwnedFd::from_raw_fd(received) });
            }
        }
        header = unsafe { libc::CMSG_NXTHDR(&message, header) };
    }
    ensure!(
        ancillary_valid
            && message.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) == 0
            && length as usize <= size,
        "truncated or invalid verified record"
    );
    ensure!(length != 0, "verified producer disconnected");
    bytes.truncate(length as usize);
    Ok((bytes, descriptors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs, mem,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{ffi::OsStrExt, fs::MetadataExt, net::UnixDatagram},
        },
        sync::atomic::{AtomicU64, Ordering},
        thread,
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        directory: std::path::PathBuf,
        raw: UnixDatagram,
        listener: OwnedFd,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "cul-verified-channel-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let raw = UnixDatagram::bind(directory.join("raw")).unwrap();
            let fd = unsafe {
                libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0)
            };
            assert!(fd >= 0);
            let listener = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
            address.sun_family = libc::AF_UNIX as _;
            let path = directory.join("identity");
            for (destination, source) in
                address.sun_path.iter_mut().zip(path.as_os_str().as_bytes())
            {
                *destination = *source as _;
            }
            assert_eq!(
                unsafe {
                    libc::bind(
                        fd,
                        (&address as *const libc::sockaddr_un).cast(),
                        mem::size_of_val(&address) as _,
                    )
                },
                0
            );
            assert_eq!(unsafe { libc::listen(fd, 1) }, 0);
            Self {
                directory,
                raw,
                listener,
            }
        }

        fn response(&self) -> [u8; 96] {
            let metadata = fs::metadata(self.directory.join("raw")).unwrap();
            let mut bytes = [0; 96];
            bytes[..8].copy_from_slice(b"YDOTID1\0");
            bytes[8..10].copy_from_slice(&1_u16.to_le_bytes());
            bytes[12..16].copy_from_slice(&96_u32.to_le_bytes());
            bytes[16..32].fill(7);
            bytes[32..39].copy_from_slice(b"input99");
            bytes[64..72].copy_from_slice(&metadata.dev().to_le_bytes());
            bytes[72..80].copy_from_slice(&metadata.ino().to_le_bytes());
            bytes[80..84].copy_from_slice(&4096_u32.to_le_bytes());
            bytes[84..88].copy_from_slice(&1_u32.to_le_bytes());
            bytes
        }

        fn serve(&self, response: Vec<u8>, fd_count: usize) -> thread::JoinHandle<Vec<Vec<u8>>> {
            self.serve_with(response, fd_count, "normal")
        }

        fn serve_with(
            &self,
            response: Vec<u8>,
            fd_count: usize,
            reply_mode: &'static str,
        ) -> thread::JoinHandle<Vec<Vec<u8>>> {
            let listener = unsafe { libc::dup(self.listener.as_raw_fd()) };
            assert!(listener >= 0);
            thread::spawn(move || {
                let listener = unsafe { OwnedFd::from_raw_fd(listener) };
                let accepted = unsafe {
                    libc::accept4(
                        listener.as_raw_fd(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        libc::SOCK_CLOEXEC,
                    )
                };
                assert!(accepted >= 0);
                let accepted = unsafe { OwnedFd::from_raw_fd(accepted) };
                let mut request = [0; 16];
                assert_eq!(
                    unsafe {
                        libc::recv(
                            accepted.as_raw_fd(),
                            request.as_mut_ptr().cast(),
                            request.len(),
                            0,
                        )
                    },
                    16
                );
                assert_eq!(&request[..8], b"YDOTID1\0");
                if reply_mode == "identity-timeout" {
                    thread::sleep(Duration::from_millis(2200));
                }
                let mut pair = [-1; 2];
                let endpoint_type = if reply_mode == "stream" {
                    libc::SOCK_STREAM
                } else {
                    libc::SOCK_SEQPACKET
                };
                assert_eq!(
                    unsafe {
                        libc::socketpair(
                            libc::AF_UNIX,
                            endpoint_type | libc::SOCK_CLOEXEC,
                            0,
                            pair.as_mut_ptr(),
                        )
                    },
                    0
                );
                let server = unsafe { OwnedFd::from_raw_fd(pair[0]) };
                let client = unsafe { OwnedFd::from_raw_fd(pair[1]) };
                let mut iov = libc::iovec {
                    iov_base: response.as_ptr().cast_mut().cast(),
                    iov_len: response.len(),
                };
                let mut control = [0_usize; 128];
                let mut message: libc::msghdr = unsafe { mem::zeroed() };
                message.msg_iov = &mut iov;
                message.msg_iovlen = 1;
                if fd_count != 0 {
                    message.msg_control = control.as_mut_ptr().cast();
                    message.msg_controllen =
                        unsafe { libc::CMSG_SPACE((fd_count * mem::size_of::<i32>()) as _) } as _;
                    let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
                    unsafe {
                        (*header).cmsg_level = libc::SOL_SOCKET;
                        (*header).cmsg_type = libc::SCM_RIGHTS;
                        (*header).cmsg_len =
                            libc::CMSG_LEN((fd_count * mem::size_of::<i32>()) as _) as _;
                        for index in 0..fd_count {
                            libc::CMSG_DATA(header)
                                .cast::<i32>()
                                .add(index)
                                .write(client.as_raw_fd());
                        }
                    }
                }
                let sent =
                    unsafe { libc::sendmsg(accepted.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
                if reply_mode == "identity-timeout" {
                    return Vec::new();
                }
                assert_eq!(sent, response.len() as isize);
                drop(client);
                let timeout = libc::timeval {
                    tv_sec: 3,
                    tv_usec: 0,
                };
                unsafe {
                    libc::setsockopt(
                        server.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_RCVTIMEO,
                        (&timeout as *const libc::timeval).cast(),
                        mem::size_of_val(&timeout) as _,
                    )
                };
                let mut records = Vec::new();
                loop {
                    let mut record = [0; 32];
                    let length = unsafe {
                        libc::recv(
                            server.as_raw_fd(),
                            record.as_mut_ptr().cast(),
                            record.len(),
                            0,
                        )
                    };
                    if length <= 0 {
                        break;
                    }
                    records.push(record[..length as usize].to_vec());
                    if reply_mode == "disconnect" && record[4] == 1 {
                        break;
                    }
                    if record[4] == 1 {
                        if reply_mode == "timeout" {
                            thread::sleep(Duration::from_millis(2200));
                        }
                        if reply_mode == "delayed" {
                            thread::sleep(Duration::from_millis(50));
                        }
                    }
                    let mut reply = [0; 16];
                    reply[..4].copy_from_slice(b"YDP1");
                    let op = u16::from_le_bytes([record[4], record[5]]) | 0x8000;
                    reply[4..6].copy_from_slice(&op.to_le_bytes());
                    reply[8..12].copy_from_slice(&record[8..12]);
                    if record[4] == 1 {
                        if reply_mode == "wrong-sequence" {
                            reply[8] += 1;
                        }
                        if reply_mode == "revoke" {
                            reply[4..6].copy_from_slice(&0xffff_u16.to_le_bytes());
                            reply[6..8].copy_from_slice(&2_u16.to_le_bytes());
                            reply[12..16].copy_from_slice(&6_u32.to_le_bytes());
                        }
                        if reply_mode == "kernel-error" {
                            reply[6..8].copy_from_slice(&3_u16.to_le_bytes());
                            reply[12..16].copy_from_slice(&9_u32.to_le_bytes());
                        }
                    }
                    unsafe {
                        libc::send(
                            server.as_raw_fd(),
                            reply.as_ptr().cast(),
                            reply.len(),
                            libc::MSG_NOSIGNAL,
                        )
                    };
                    if op != 0x8001 {
                        break;
                    }
                }
                records
            })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn identity_open_pins_the_transferred_channel_without_writing_raw_socket() {
        let fixture = Fixture::new();
        let producer = fixture.serve(fixture.response().to_vec(), 1);
        let channel = Channel::open(
            &fixture.directory.join("identity"),
            &fixture.directory.join("raw"),
        );
        assert!(
            channel.is_ok(),
            "valid protocol handshake must open: {:?}",
            channel.err()
        );
        drop(channel);
        let records = producer.join().unwrap();
        assert!(records
            .iter()
            .all(|record| u16::from_le_bytes([record[4], record[5]]) == 3));
        fixture.raw.set_nonblocking(true).unwrap();
        assert_eq!(
            fixture.raw.recv(&mut [0; 32]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn malformed_identity_and_extra_fds_never_submit_a_stroke() {
        for case in 0..10 {
            let fixture = Fixture::new();
            let mut response = fixture.response().to_vec();
            let mut fd_count = 1;
            match case {
                0 => {
                    response.pop();
                }
                1 => response[88] = 1,
                2 => response[40] = 1,
                3 => response[84] = 3,
                4 => fd_count = 2,
                5 => response[72] ^= 1,
                6 => fd_count = 0,
                7 => fd_count = 128,
                8 => response[8] = 2,
                9 => response[12] = 16,
                _ => unreachable!(),
            }
            let producer = fixture.serve(response, fd_count);
            assert!(
                Channel::open(
                    &fixture.directory.join("identity"),
                    &fixture.directory.join("raw")
                )
                .is_err(),
                "case {case}"
            );
            assert!(
                producer.join().unwrap().is_empty(),
                "case {case} submitted input"
            );
        }
    }

    #[test]
    fn complete_strokes_are_sequenced_and_finished_only_on_the_pinned_channel() {
        let fixture = Fixture::new();
        let producer = fixture.serve(fixture.response().to_vec(), 1);
        let mut channel = Channel::open(
            &fixture.directory.join("identity"),
            &fixture.directory.join("raw"),
        )
        .unwrap();
        channel
            .stroke(
                Stroke {
                    keycode: 30,
                    left_shift: true,
                },
                &mut || false,
            )
            .unwrap();
        channel
            .stroke(
                Stroke {
                    keycode: 57,
                    left_shift: false,
                },
                &mut || false,
            )
            .unwrap();
        channel.finish(&mut || false).unwrap();
        drop(channel);
        let records = producer.join().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(&records[0][4..14], &[1, 0, 1, 0, 1, 0, 0, 0, 30, 0]);
        assert_eq!(&records[1][4..14], &[1, 0, 0, 0, 2, 0, 0, 0, 57, 0]);
        assert_eq!(&records[2][4..14], &[2, 0, 0, 0, 3, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn replacement_and_pre_submission_cancellation_cannot_reach_a_stroke() {
        for replace in [false, true] {
            let fixture = Fixture::new();
            let producer = fixture.serve(fixture.response().to_vec(), 1);
            let mut channel = Channel::open(
                &fixture.directory.join("identity"),
                &fixture.directory.join("raw"),
            )
            .unwrap();
            let _replacement = if replace {
                fs::remove_file(fixture.directory.join("raw")).unwrap();
                Some(UnixDatagram::bind(fixture.directory.join("raw")).unwrap())
            } else {
                None
            };
            assert!(channel
                .stroke(
                    Stroke {
                        keycode: 30,
                        left_shift: true
                    },
                    &mut || !replace
                )
                .is_err());
            assert!(!channel.may_have_submitted);
            drop(channel);
            assert!(producer.join().unwrap().iter().all(|record| record[4] == 3));
        }
    }

    #[test]
    fn wrong_acknowledgement_revocation_and_disconnect_forbid_replay() {
        for mode in ["wrong-sequence", "revoke", "disconnect", "kernel-error"] {
            let fixture = Fixture::new();
            let producer = fixture.serve_with(fixture.response().to_vec(), 1, mode);
            let mut channel = Channel::open(
                &fixture.directory.join("identity"),
                &fixture.directory.join("raw"),
            )
            .unwrap();
            assert!(
                channel
                    .stroke(
                        Stroke {
                            keycode: 30,
                            left_shift: false
                        },
                        &mut || false
                    )
                    .is_err(),
                "mode {mode}"
            );
            assert!(channel.may_have_submitted);
            drop(channel);
            let records = producer.join().unwrap();
            assert_eq!(
                records.iter().filter(|record| record[4] == 1).count(),
                1,
                "mode {mode}"
            );
        }
    }

    #[test]
    fn cancellation_drains_the_pending_ack_and_confirms_channel_cleanup() {
        let fixture = Fixture::new();
        let producer = fixture.serve_with(fixture.response().to_vec(), 1, "delayed");
        let mut channel = Channel::open(
            &fixture.directory.join("identity"),
            &fixture.directory.join("raw"),
        )
        .unwrap();
        let started = Instant::now();
        assert!(channel
            .stroke(
                Stroke {
                    keycode: 30,
                    left_shift: true
                },
                &mut || started.elapsed() >= Duration::from_millis(10)
            )
            .is_err());
        assert!(channel.may_have_submitted);
        channel
            .cancel()
            .expect("pending stroke ACK must not obscure cancellation ACK");
        drop(channel);
        let records = producer.join().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0][4], 1);
        assert_eq!(records[1][4], 3);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn acknowledgement_timeout_remains_bounded_and_never_replays() {
        let fixture = Fixture::new();
        let producer = fixture.serve_with(fixture.response().to_vec(), 1, "timeout");
        let mut channel = Channel::open(
            &fixture.directory.join("identity"),
            &fixture.directory.join("raw"),
        )
        .unwrap();
        let started = Instant::now();
        assert!(channel
            .stroke(
                Stroke {
                    keycode: 30,
                    left_shift: false
                },
                &mut || false
            )
            .is_err());
        assert!(started.elapsed() < Duration::from_millis(2500));
        assert!(channel.may_have_submitted);
        channel.cancel().unwrap();
        drop(channel);
        let records = producer.join().unwrap();
        assert_eq!(records.iter().filter(|record| record[4] == 1).count(), 1);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn identity_errors_invalid_endpoint_and_timeout_never_submit() {
        for status in 1..=3_u16 {
            let fixture = Fixture::new();
            let mut response = fixture.response();
            response[10..12].copy_from_slice(&status.to_le_bytes());
            response[16..].fill(0);
            let producer = fixture.serve(response.to_vec(), 0);
            assert!(Channel::open(
                &fixture.directory.join("identity"),
                &fixture.directory.join("raw")
            )
            .is_err());
            assert!(producer.join().unwrap().is_empty());
        }
        for mode in ["stream", "identity-timeout"] {
            let fixture = Fixture::new();
            let producer = fixture.serve_with(fixture.response().to_vec(), 1, mode);
            let started = Instant::now();
            assert!(Channel::open(
                &fixture.directory.join("identity"),
                &fixture.directory.join("raw")
            )
            .is_err());
            assert!(started.elapsed() < Duration::from_millis(2500));
            assert!(producer.join().unwrap().is_empty());
        }
    }
}
