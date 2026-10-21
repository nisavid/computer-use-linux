//! A separate, bounded D-Bus connection for untrusted accessibility providers.
//!
//! A method timeout alone cannot bound an incoming D-Bus allocation: zbus reads
//! the advertised frame length and allocates its buffer before dispatching the
//! reply. Inspect the fixed header first, before its normal decoder runs.

use std::{io, os::fd::OwnedFd, process::Stdio, time::Duration};

use anyhow::{bail, Context, Result};
use atspi::proxy::bus::BusProxy;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zbus::{
    address::transport::{TcpTransportFamily, Transport, UnixSocket},
    connection::{
        socket::{BoxedSplit, ReadHalf, Split},
        Builder,
    },
    export::async_trait::async_trait,
    fdo::ConnectionCredentials,
    Address, AuthMechanism, Connection, Message,
};

pub(crate) const MAX_MESSAGE_BYTES: usize = 256 * 1024;
const PRIMARY_HEADER_BYTES: usize = 16;
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024;
const MAX_FRAME_FDS: usize = 64;
const METHOD_TIMEOUT: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) async fn connect() -> Result<Connection> {
    connect_with_method_timeout(Some(METHOD_TIMEOUT)).await
}

/// Mutations retain the baseline completion semantics: connection and address
/// discovery are bounded, while the actual mutation may complete in its own time.
pub(crate) async fn connect_for_mutation() -> Result<Connection> {
    connect_with_method_timeout(None).await
}

async fn connect_with_method_timeout(method_timeout: Option<Duration>) -> Result<Connection> {
    let session = connect_address(&Address::session()?)
        .await
        .context("connecting to the session bus")?;
    let address = BusProxy::new(&session)
        .await?
        .get_address()
        .await
        .context("resolving the accessibility bus")?;
    connect_address_with_method_timeout(&address.parse()?, method_timeout).await
}

pub(crate) async fn connect_address(address: &Address) -> Result<Connection> {
    connect_address_with_method_timeout(address, Some(METHOD_TIMEOUT)).await
}

async fn connect_address_with_method_timeout(
    address: &Address,
    method_timeout: Option<Duration>,
) -> Result<Connection> {
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let (socket, address) = open_socket(address).await?;
        capped_builder(
            socket,
            address.guid().map(|guid| guid.as_str().to_owned()),
            method_timeout,
        )
        .build()
        .await
        .map_err(Into::into)
    })
    .await
    .context("accessibility bus connection timed out")?
}

fn capped_builder(
    socket: BoxedSplit,
    expected_guid: Option<String>,
    method_timeout: Option<Duration>,
) -> Builder<'static> {
    let (read, write) = socket.take();
    let read: Box<dyn ReadHalf> = Box::new(CappedRead {
        inner: DeferredRead {
            inner: read,
            header_fds: Vec::new(),
            frame_fds: 0,
        },
        handshake_bytes: 0,
        auth_line: Vec::new(),
        expected_guid,
    });
    let builder = Builder::socket(Split::new(read, write)).max_queued(8);
    match method_timeout {
        Some(timeout) => builder.method_timeout(timeout),
        None => builder,
    }
}

/// Establish only the raw transport. Every successful path is wrapped before
/// zbus authenticates or reads a message; unknown transports fail closed.
async fn open_socket(address: &Address) -> Result<(BoxedSplit, Address)> {
    let mut address = address.clone();
    for _ in 0..4 {
        let socket: BoxedSplit = match address.transport() {
            Transport::Unix(unix) => {
                let stream = match unix.path() {
                    UnixSocket::File(path) => tokio::net::UnixStream::connect(path).await?,
                    #[cfg(target_os = "linux")]
                    UnixSocket::Abstract(name) => {
                        use std::os::unix::ffi::OsStrExt;
                        // Tokio recognizes a leading NUL as an abstract Unix
                        // address, and connects without a blocking worker.
                        let mut path = vec![0];
                        path.extend_from_slice(name.as_bytes());
                        tokio::net::UnixStream::connect(std::ffi::OsStr::from_bytes(&path)).await?
                    }
                    _ => bail!("unsupported accessibility bus Unix address"),
                };
                stream.into()
            }
            Transport::Tcp(tcp) => {
                let mut last_error =
                    io::Error::new(io::ErrorKind::AddrNotAvailable, "no matching TCP address");
                let mut connected = None;
                for endpoint in tokio::net::lookup_host((tcp.host(), tcp.port())).await? {
                    if tcp.family().is_some_and(|family| match family {
                        TcpTransportFamily::Ipv4 => !endpoint.is_ipv4(),
                        TcpTransportFamily::Ipv6 => !endpoint.is_ipv6(),
                    }) {
                        continue;
                    }
                    match tokio::net::TcpStream::connect(endpoint).await {
                        Ok(stream) => {
                            connected = Some(stream);
                            break;
                        }
                        Err(error) => last_error = error,
                    }
                }
                let mut stream = connected.ok_or(last_error)?;
                if let Some(nonce_file) = tcp.nonce_file() {
                    use std::os::unix::ffi::OsStrExt;
                    // The nonce is trusted local configuration, but still avoid
                    // reading an arbitrarily large file into memory.
                    let mut nonce = tokio::fs::File::open(std::ffi::OsStr::from_bytes(nonce_file))
                        .await?
                        .take(MAX_HANDSHAKE_BYTES as u64 + 1);
                    let mut bytes = Vec::new();
                    nonce.read_to_end(&mut bytes).await?;
                    if bytes.len() > MAX_HANDSHAKE_BYTES {
                        bail!("accessibility bus nonce file exceeds limit");
                    }
                    stream.write_all(&bytes).await?;
                }
                stream.into()
            }
            Transport::Unixexec(exec) => {
                let mut command = tokio::process::Command::new(exec.path());
                command
                    .args(exec.args())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit());
                if let Some(arg0) = exec.arg0() {
                    command.arg0(arg0);
                }
                let mut child = command.spawn()?;
                let read: Box<dyn ReadHalf> =
                    Box::new(child.stdout.take().context("missing bus stdout")?);
                let write: Box<dyn zbus::connection::socket::WriteHalf> =
                    Box::new(child.stdin.take().context("missing bus stdin")?);
                Split::new(read, write)
            }
            Transport::Ibus(_) => {
                let mut child = tokio::process::Command::new("ibus")
                    .arg("address")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()?;
                let mut output = child
                    .stdout
                    .take()
                    .context("missing ibus stdout")?
                    .take(4097);
                let mut bytes = Vec::new();
                output.read_to_end(&mut bytes).await?;
                if bytes.len() > 4096 {
                    bail!("ibus address exceeds limit");
                }
                if !child.wait().await?.success() {
                    bail!("ibus address command failed");
                }
                let mut resolved: Address = std::str::from_utf8(&bytes)?.trim().parse()?;
                if let Some(guid) = address.guid() {
                    resolved = resolved.set_guid(guid.to_owned())?;
                }
                address = resolved;
                continue;
            }
            _ => bail!(
                "unsupported accessibility bus transport: {}",
                address.transport()
            ),
        };
        return Ok((socket, address));
    }
    bail!("too many indirect accessibility bus addresses")
}

/// Descriptors received with the fixed header must reach zbus as descriptors
/// of the current message. Its `already_received_fds` argument instead denotes
/// descriptors buffered across messages, so deliver ours on the next body read.
#[derive(Debug)]
struct DeferredRead {
    inner: Box<dyn ReadHalf>,
    header_fds: Vec<OwnedFd>,
    frame_fds: usize,
}

#[async_trait]
impl ReadHalf for DeferredRead {
    async fn recvmsg(&mut self, buf: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
        let (read, fds) = self.inner.recvmsg(buf).await?;
        self.frame_fds = self.frame_fds.saturating_add(fds.len());
        if self.frame_fds > MAX_FRAME_FDS {
            return Err(limit_error(
                "too many accessibility message file descriptors",
            ));
        }
        let mut header_fds = std::mem::take(&mut self.header_fds);
        header_fds.extend(fds);
        Ok((read, header_fds))
    }
}

#[derive(Debug)]
struct CappedRead {
    inner: DeferredRead,
    handshake_bytes: usize,
    auth_line: Vec<u8>,
    expected_guid: Option<String>,
}

#[async_trait]
impl ReadHalf for CappedRead {
    async fn receive_message(
        &mut self,
        seq: u64,
        bytes: &mut Vec<u8>,
        fds: &mut Vec<OwnedFd>,
    ) -> zbus::Result<Message> {
        if bytes.len() > MAX_MESSAGE_BYTES || fds.len() > MAX_FRAME_FDS {
            return Err(zbus::Error::ExcessData);
        }
        self.inner.frame_fds = fds.len();
        if bytes.len() < PRIMARY_HEADER_BYTES {
            let mut position = bytes.len();
            bytes.resize(PRIMARY_HEADER_BYTES, 0);
            while position < PRIMARY_HEADER_BYTES {
                let (read, incoming_fds) = self.inner.inner.recvmsg(&mut bytes[position..]).await?;
                if read == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "incomplete D-Bus header",
                    )
                    .into());
                }
                self.inner.frame_fds = self.inner.frame_fds.saturating_add(incoming_fds.len());
                if self.inner.frame_fds > MAX_FRAME_FDS {
                    return Err(zbus::Error::ExcessData);
                }
                self.inner.header_fds.extend(incoming_fds);
                position += read;
            }
        }
        let total = frame_length(&bytes[..PRIMARY_HEADER_BYTES])?;
        if total == PRIMARY_HEADER_BYTES && !self.inner.header_fds.is_empty() {
            return Err(zbus::Error::ExcessData);
        }
        self.inner.receive_message(seq, bytes, fds).await
    }

    async fn recvmsg(&mut self, buf: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
        let remaining = MAX_HANDSHAKE_BYTES.saturating_sub(self.handshake_bytes);
        if remaining == 0 {
            return Err(limit_error(
                "accessibility bus authentication exceeds limit",
            ));
        }
        // Do not let an authentication read consume binary message bytes.
        // Besides keeping all frames behind the primary-header check, this
        // avoids zbus's separate buffered-descriptor path during the handshake.
        let length = buf.len().min(remaining).min(1);
        let (read, fds) = self.inner.inner.recvmsg(&mut buf[..length]).await?;
        self.handshake_bytes += read;
        if !fds.is_empty() {
            return Err(limit_error("file descriptors in D-Bus authentication"));
        }
        // Builder::socket cannot be given an expected server GUID. Retain the
        // same check as Builder::address without replacing zbus authentication.
        for byte in &buf[..read] {
            self.auth_line.push(*byte);
            if *byte == b'\n' {
                if self.auth_line.len() < 2 {
                    return Err(limit_error("invalid authentication line"));
                }
                if let Some(expected) = &self.expected_guid {
                    let line = std::str::from_utf8(&self.auth_line)
                        .map_err(|_| limit_error("invalid authentication UTF-8"))?;
                    // Match zbus's command grammar, including leading ASCII
                    // whitespace and whitespace other than a single space.
                    let mut words = line.split_ascii_whitespace();
                    if words.next() == Some("OK") && words.next() != Some(expected.as_str()) {
                        return Err(limit_error("accessibility bus server GUID mismatch"));
                    }
                }
                self.auth_line.clear();
            }
        }
        Ok((read, fds))
    }

    fn can_pass_unix_fd(&self) -> bool {
        self.inner.inner.can_pass_unix_fd()
    }

    async fn peer_credentials(&mut self) -> io::Result<ConnectionCredentials> {
        self.inner.inner.peer_credentials().await
    }

    fn auth_mechanism(&self) -> AuthMechanism {
        self.inner.inner.auth_mechanism()
    }
}

fn frame_length(header: &[u8]) -> zbus::Result<usize> {
    let read_u32 = match header[0] {
        b'l' => u32::from_le_bytes,
        b'B' => u32::from_be_bytes,
        _ => return Err(zbus::Error::IncorrectEndian),
    };
    let body = read_u32(header[4..8].try_into().unwrap()) as usize;
    let fields = read_u32(header[12..16].try_into().unwrap()) as usize;
    let length = PRIMARY_HEADER_BYTES
        .checked_add(fields)
        .and_then(|header| header.checked_add((8 - header % 8) % 8))
        .and_then(|header| header.checked_add(body))
        .filter(|length| *length <= MAX_MESSAGE_BYTES)
        .ok_or(zbus::Error::ExcessData)?;
    Ok(length)
}

fn limit_error(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Real zbus authentication and message dispatch over bounded in-memory byte
/// streams, for provider tests on hosts that forbid creating kernel sockets.
#[cfg(test)]
pub(crate) async fn test_connection_pair() -> Result<(Connection, Connection)> {
    test_connection_pair_with_method_timeout(Some(METHOD_TIMEOUT)).await
}

#[cfg(test)]
pub(crate) async fn test_mutation_connection_pair() -> Result<(Connection, Connection)> {
    test_connection_pair_with_method_timeout(None).await
}

#[cfg(test)]
async fn test_connection_pair_with_method_timeout(
    method_timeout: Option<Duration>,
) -> Result<(Connection, Connection)> {
    use zbus::connection::socket::WriteHalf;

    #[derive(Debug)]
    struct TestRead(tokio::io::ReadHalf<tokio::io::DuplexStream>);
    #[async_trait]
    impl ReadHalf for TestRead {
        async fn recvmsg(&mut self, buffer: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
            Ok((self.0.read(buffer).await?, Vec::new()))
        }
        fn auth_mechanism(&self) -> AuthMechanism {
            AuthMechanism::Anonymous
        }
    }
    #[derive(Debug)]
    struct TestWrite(tokio::io::WriteHalf<tokio::io::DuplexStream>);
    #[async_trait]
    impl WriteHalf for TestWrite {
        async fn sendmsg(
            &mut self,
            buffer: &[u8],
            fds: &[std::os::fd::BorrowedFd<'_>],
        ) -> io::Result<usize> {
            if !fds.is_empty() {
                return Err(limit_error("test stream does not pass descriptors"));
            }
            self.0.write(buffer).await
        }
        async fn close(&mut self) -> io::Result<()> {
            self.0.shutdown().await
        }
    }
    fn socket(stream: tokio::io::DuplexStream) -> BoxedSplit {
        let (read, write) = tokio::io::split(stream);
        Split::new(
            Box::new(TestRead(read)) as Box<dyn ReadHalf>,
            Box::new(TestWrite(write)) as Box<dyn WriteHalf>,
        )
    }
    struct Ready;
    #[zbus::interface(name = "org.test.Ready")]
    impl Ready {
        fn ping(&self) -> bool {
            true
        }
    }
    const GUID: &str = "0123456789abcdef0123456789abcdef";
    let (server, client) = tokio::io::duplex(16 * 1024);
    let server = Builder::socket(socket(server))
        .server(zbus::Guid::from_static_str(GUID)?)?
        .auth_mechanism(AuthMechanism::Anonymous)
        .p2p()
        // Model both the registry/provider and the bus PID lookup service.
        .unique_name("org.freedesktop.DBus")?
        // Builder waits for object-server subscription before returning.
        .serve_at("/org/test/Ready", Ready)?;
    let client = capped_builder(socket(client), Some(GUID.to_owned()), method_timeout)
        .auth_mechanism(AuthMechanism::Anonymous)
        .p2p()
        .unique_name(":1.43")?;
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        Ok(tokio::try_join!(server.build(), client.build())?)
    })
    .await
    .context("test authentication timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use zbus::zvariant::Endian;

    #[derive(Debug)]
    struct ScriptedRead {
        data: Vec<u8>,
        offset: Arc<AtomicUsize>,
        chunk: usize,
        stop: usize,
        fds: Vec<OwnedFd>,
    }

    #[async_trait]
    impl ReadHalf for ScriptedRead {
        async fn recvmsg(&mut self, buf: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
            let offset = self.offset.load(Ordering::Relaxed);
            assert!(offset < self.stop, "a rejected frame's body was read");
            let length = buf.len().min(self.chunk).min(self.data.len() - offset);
            buf[..length].copy_from_slice(&self.data[offset..offset + length]);
            self.offset.store(offset + length, Ordering::Relaxed);
            Ok((length, std::mem::take(&mut self.fds)))
        }
    }

    fn capped(inner: Box<dyn ReadHalf>) -> CappedRead {
        CappedRead {
            inner: DeferredRead {
                inner,
                header_fds: Vec::new(),
                frame_fds: 0,
            },
            handshake_bytes: 0,
            auth_line: Vec::new(),
            expected_guid: None,
        }
    }

    fn scripted(data: Vec<u8>, chunk: usize, stop: usize) -> (CappedRead, Arc<AtomicUsize>) {
        let offset = Arc::new(AtomicUsize::new(0));
        (
            capped(Box::new(ScriptedRead {
                data,
                offset: offset.clone(),
                chunk,
                stop,
                fds: Vec::new(),
            })),
            offset,
        )
    }

    fn primary_header(endian: Endian, body: u32, fields: u32) -> Vec<u8> {
        let mut bytes = vec![0; PRIMARY_HEADER_BYTES];
        bytes[0] = match endian {
            Endian::Little => b'l',
            Endian::Big => b'B',
        };
        bytes[1] = 2;
        bytes[3] = 1;
        let encode = match endian {
            Endian::Little => u32::to_le_bytes,
            Endian::Big => u32::to_be_bytes,
        };
        bytes[4..8].copy_from_slice(&encode(body));
        bytes[8..12].copy_from_slice(&encode(1));
        bytes[12..16].copy_from_slice(&encode(fields));
        bytes
    }

    #[tokio::test]
    async fn oversized_frames_are_rejected_before_any_body_read() {
        for endian in [Endian::Little, Endian::Big] {
            for (body, fields) in [
                (MAX_MESSAGE_BYTES as u32, 0),
                (0, u32::MAX),
                (u32::MAX, u32::MAX),
            ] {
                let (mut read, offset) = scripted(
                    primary_header(endian, body, fields),
                    3,
                    PRIMARY_HEADER_BYTES,
                );
                let mut buffered = Vec::new();
                assert!(matches!(
                    read.receive_message(0, &mut buffered, &mut Vec::new())
                        .await,
                    Err(zbus::Error::ExcessData)
                ));
                assert_eq!(offset.load(Ordering::Relaxed), PRIMARY_HEADER_BYTES);
                assert_eq!(buffered.len(), PRIMARY_HEADER_BYTES);
                assert!(buffered.capacity() <= PRIMARY_HEADER_BYTES);
            }
        }
    }

    #[tokio::test]
    async fn oversized_buffered_header_is_rejected_without_socket_read() {
        let (mut read, _) = scripted(Vec::new(), 1, 0);
        let mut buffered = primary_header(Endian::Little, 0, MAX_MESSAGE_BYTES as u32);
        assert!(matches!(
            read.receive_message(0, &mut buffered, &mut Vec::new())
                .await,
            Err(zbus::Error::ExcessData)
        ));
    }

    #[test]
    fn frame_limit_includes_header_fields_and_body_alignment() {
        for endian in [Endian::Little, Endian::Big] {
            assert_eq!(
                frame_length(&primary_header(endian, (MAX_MESSAGE_BYTES - 24) as u32, 1)).unwrap(),
                MAX_MESSAGE_BYTES
            );
            assert!(matches!(
                frame_length(&primary_header(endian, (MAX_MESSAGE_BYTES - 23) as u32, 1)),
                Err(zbus::Error::ExcessData)
            ));
        }
        let mut malformed = primary_header(Endian::Little, 0, 0);
        malformed[0] = b'x';
        assert!(matches!(
            frame_length(&malformed),
            Err(zbus::Error::IncorrectEndian)
        ));
    }

    #[tokio::test]
    async fn normal_messages_in_both_byte_orders_decode_after_partial_header_reads() {
        for endian in [Endian::Little, Endian::Big] {
            let message = Message::signal("/org/test", "org.test.Interface", "Changed")
                .unwrap()
                .endian(endian)
                .build(&"hello")
                .unwrap();
            let (mut read, offset) = scripted(message.data().to_vec(), 3, usize::MAX);
            let received = read
                .receive_message(17, &mut Vec::new(), &mut Vec::new())
                .await
                .unwrap();
            assert_eq!(received.body().deserialize::<String>().unwrap(), "hello");
            assert_eq!(offset.load(Ordering::Relaxed), message.data().len());
        }
    }

    #[tokio::test]
    async fn descriptors_received_with_primary_header_are_preserved() {
        use std::os::fd::AsFd;
        let file = std::fs::File::open("/dev/null").unwrap();
        let message = Message::signal("/org/test", "org.test.Interface", "Changed")
            .unwrap()
            .build(&zbus::zvariant::Fd::from(file.as_fd()))
            .unwrap();
        let mut read = capped(Box::new(ScriptedRead {
            data: message.data().to_vec(),
            offset: Arc::new(AtomicUsize::new(0)),
            chunk: usize::MAX,
            stop: usize::MAX,
            fds: vec![file.into()],
        }));
        let received = read
            .receive_message(0, &mut Vec::new(), &mut Vec::new())
            .await
            .unwrap();
        let descriptor: zbus::zvariant::OwnedFd = received.body().deserialize().unwrap();
        assert!(std::fs::File::from(OwnedFd::from(descriptor))
            .metadata()
            .is_ok());
    }

    #[tokio::test]
    async fn authentication_is_bounded_and_does_not_read_binary_messages() {
        let auth = b"OK 0123456789abcdef0123456789abcdef\r\n";
        let message = Message::signal("/org/test", "org.test.Interface", "Changed")
            .unwrap()
            .build(&"hello")
            .unwrap();
        let mut bytes = auth.to_vec();
        bytes.extend_from_slice(message.data());
        let (mut read, offset) = scripted(bytes, usize::MAX, usize::MAX);
        read.expected_guid = Some("0123456789abcdef0123456789abcdef".to_owned());
        let mut buffer = [0; 1024];
        for _ in 0..auth.len() {
            assert_eq!(read.recvmsg(&mut buffer).await.unwrap().0, 1);
        }
        assert_eq!(offset.load(Ordering::Relaxed), auth.len());
        let received = read
            .receive_message(0, &mut Vec::new(), &mut Vec::new())
            .await
            .unwrap();
        assert_eq!(received.body().deserialize::<String>().unwrap(), "hello");

        let (mut read, offset) =
            scripted(vec![b'x'; MAX_HANDSHAKE_BYTES + 1], usize::MAX, usize::MAX);
        for _ in 0..MAX_HANDSHAKE_BYTES {
            read.recvmsg(&mut buffer).await.unwrap();
        }
        assert!(read.recvmsg(&mut buffer).await.is_err());
        assert_eq!(offset.load(Ordering::Relaxed), MAX_HANDSHAKE_BYTES);
    }

    #[tokio::test]
    async fn address_guid_mismatch_is_rejected() {
        for response in [
            "DATA a\r\nOK ffffffffffffffffffffffffffffffff\r\n",
            "OK\tffffffffffffffffffffffffffffffff\r\n",
            " \tOK\tffffffffffffffffffffffffffffffff\r\n",
        ] {
            let (mut read, _) = scripted(response.as_bytes().to_vec(), usize::MAX, usize::MAX);
            read.expected_guid = Some("0123456789abcdef0123456789abcdef".to_owned());
            let mut buffer = [0; 1024];
            loop {
                match read.recvmsg(&mut buffer).await {
                    Ok((0, _)) => panic!("GUID mismatch was accepted: {response:?}"),
                    Ok(_) => {}
                    Err(error) => {
                        assert!(error.to_string().contains("GUID mismatch"));
                        break;
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn matching_address_guid_accepts_zbus_ascii_whitespace_grammar() {
        for response in [
            "OK 0123456789abcdef0123456789abcdef\r\n",
            "OK\t0123456789abcdef0123456789abcdef\r\n",
            " \tOK\t0123456789abcdef0123456789abcdef\r\n",
        ] {
            let (mut read, offset) = scripted(response.as_bytes().to_vec(), usize::MAX, usize::MAX);
            read.expected_guid = Some("0123456789abcdef0123456789abcdef".to_owned());
            let mut buffer = [0; 1024];
            for _ in 0..response.len() {
                assert_eq!(read.recvmsg(&mut buffer).await.unwrap().0, 1);
            }
            assert_eq!(offset.load(Ordering::Relaxed), response.len());
        }
    }
}
