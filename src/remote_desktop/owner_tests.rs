use super::*;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::Mutex,
};
use zbus::{message::Type, Message, MessageStream};
const SESSION: &str = "/org/freedesktop/portal/desktop/session/test/session";
struct TestBus {
    child: Child,
    address: String,
}
impl TestBus {
    fn start() -> Self {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("tests require dbus-daemon");
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        if address.trim().is_empty() {
            let output = child.wait_with_output().unwrap();
            panic!(
                "private D-Bus startup failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Self {
            child,
            address: address.trim().to_owned(),
        }
    }
    async fn connect(&self) -> Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .method_timeout(Duration::from_secs(3))
            .build()
            .await
            .unwrap()
    }
}
impl Drop for TestBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
struct Portal {
    connection: Connection,
    calls: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Portal {
    async fn start(bus: &TestBus) -> Self {
        let connection = bus.connect().await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::clone(&calls);
        let server = connection.clone();
        let mut incoming = MessageStream::from(&connection);
        let task = tokio::spawn(async move {
            while let Some(message) = incoming.next().await {
                let message = message.unwrap();
                if message.message_type() != Type::MethodCall {
                    continue;
                }
                let header = message.header();
                let member = header.member().unwrap().as_str();
                received.lock().unwrap().push(member.to_owned());
                let options: Option<HashMap<String, OwnedVariant>> = match member {
                    "CreateSession" => Some(message.body().deserialize().unwrap()),
                    "SelectDevices" | "SelectSources" => {
                        let (_, opts): (OwnedObjectPath, HashMap<String, OwnedVariant>) =
                            message.body().deserialize().unwrap();
                        Some(opts)
                    }
                    "Start" => {
                        let (_, _, opts): (OwnedObjectPath, String, HashMap<String, OwnedVariant>) =
                            message.body().deserialize().unwrap();
                        Some(opts)
                    }
                    _ => None,
                };
                if let Some(options) = options {
                    let token: String = options["handle_token"]
                        .try_clone()
                        .unwrap()
                        .try_into()
                        .unwrap();
                    let path = OwnedObjectPath::try_from(request_path(
                        header.sender().unwrap().as_str(),
                        &token,
                    ))
                    .unwrap();
                    server
                        .send(&Message::method_return(&header).build(&path).unwrap())
                        .await
                        .unwrap();
                    let mut results = HashMap::new();
                    if member == "CreateSession" {
                        results.insert(
                            "session_handle",
                            OwnedVariant::try_from(Variant::from(SESSION)).unwrap(),
                        );
                    }
                    if member == "Start" {
                        results.insert(
                            "devices",
                            OwnedVariant::from(DEVICE_KEYBOARD | DEVICE_POINTER),
                        );
                    }
                    server
                        .emit_signal(
                            None::<&str>,
                            path.as_str(),
                            PORTAL_REQUEST_INTERFACE,
                            "Response",
                            &(0_u32, results),
                        )
                        .await
                        .unwrap();
                } else {
                    server
                        .send(&Message::method_return(&header).build(&()).unwrap())
                        .await
                        .unwrap();
                }
            }
        });
        Self {
            connection,
            calls,
            task,
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
impl Drop for Portal {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn keyboard(connection: &PortalConnection) -> PortalKeyboardSession {
    PortalKeyboardSession {
        connection: connection.clone(),
        session_handle: OwnedObjectPath::try_from(SESSION).unwrap(),
        input_lock: Arc::new(AsyncMutex::new(())),
        valid: Arc::new(AtomicBool::new(true)),
    }
}
fn pointer(connection: &PortalConnection) -> PortalPointerSession {
    PortalPointerSession {
        connection: connection.clone(),
        session_handle: OwnedObjectPath::try_from(SESSION).unwrap(),
        streams: Vec::new(),
        desktop_layout: None,
        input_lock: Arc::new(AsyncMutex::new(())),
        valid: Arc::new(AtomicBool::new(true)),
    }
}
async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fixture condition timed out");
}
#[tokio::test]
async fn portal_owner_change_revokes_cached_input_and_pins_in_flight_calls() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let bus = TestBus::start();
        let original = Portal::start(&bus).await;
        original
            .connection
            .request_name(PORTAL_DESKTOP_SERVICE)
            .await
            .unwrap();
        let replacement = Portal::start(&bus).await;
        let connection = PortalConnection::new(bus.connect().await).await.unwrap();
        let keyboard = keyboard(&connection);
        let cached_keyboard = keyboard.clone();
        let pointer = pointer(&connection);
        let cached_pointer = pointer.clone();
        let handle = create_remote_desktop_session(&connection).await.unwrap();
        select_keyboard_devices(&connection, &handle, None)
            .await
            .unwrap();
        select_pointer_devices(&connection, &handle, None)
            .await
            .unwrap();
        select_monitor_sources(&connection, &handle).await.unwrap();
        assert_eq!(
            start_remote_desktop_session(&connection, &handle)
                .await
                .unwrap()
                .0,
            DEVICE_KEYBOARD | DEVICE_POINTER
        );
        type_text_with_keysyms(&keyboard, &[97]).await.unwrap();
        press_keycode_chord(&keyboard, &[29], 30).await.unwrap();
        scroll(&pointer, None, ScrollDirection::Down, 1)
            .await
            .unwrap();
        let in_flight = remote_desktop_proxy(&connection).await.unwrap();
        original
            .connection
            .release_name(PORTAL_DESKTOP_SERVICE)
            .await
            .unwrap();
        replacement
            .connection
            .request_name(PORTAL_DESKTOP_SERVICE)
            .await
            .unwrap();
        wait_until(|| !cached_keyboard.is_valid() && !cached_pointer.is_valid()).await;
        assert!(type_text_with_keysyms(&cached_keyboard, &[98])
            .await
            .is_err());
        assert!(scroll(&cached_pointer, None, ScrollDirection::Down, 1)
            .await
            .is_err());
        assert!(select_monitor_sources(&connection, &handle).await.is_err());
        notify_keyboard_keysym(&in_flight, &handle, 99, KEY_RELEASED)
            .await
            .unwrap();
        close_portal_session(&connection, &handle).await;
        assert!(replacement.calls().is_empty());
        assert!(original.calls().contains(&"Close".to_owned()));
        // Positive control demonstrates the former mutable-name routing.
        let mutable = Proxy::new(
            &connection.bus,
            PORTAL_DESKTOP_SERVICE,
            PORTAL_DESKTOP_PATH,
            PORTAL_REMOTE_DESKTOP_INTERFACE,
        )
        .await
        .unwrap();
        notify_keyboard_keysym(&mutable, &handle, 100, KEY_PRESSED)
            .await
            .unwrap();
        assert_eq!(replacement.calls(), vec!["NotifyKeyboardKeysym"]);
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn portal_owner_live_check_rejects_takeover_before_monitor_runs() {
    let bus = TestBus::start();
    let original = Portal::start(&bus).await;
    original
        .connection
        .request_name(PORTAL_DESKTOP_SERVICE)
        .await
        .unwrap();
    let replacement = Portal::start(&bus).await;
    let connection = PortalConnection::new(bus.connect().await).await.unwrap();
    let session = keyboard(&connection);
    connection._monitor.0.abort();
    original
        .connection
        .release_name(PORTAL_DESKTOP_SERVICE)
        .await
        .unwrap();
    replacement
        .connection
        .request_name(PORTAL_DESKTOP_SERVICE)
        .await
        .unwrap();
    assert!(session.is_valid());
    assert!(type_text_with_keysyms(&session, &[97]).await.is_err());
    assert!(!session.is_valid());
    assert!(replacement.calls().is_empty());
}
#[tokio::test]
async fn portal_owner_release_and_reacquire_never_revives_a_session() {
    let bus = TestBus::start();
    let original = Portal::start(&bus).await;
    original
        .connection
        .request_name(PORTAL_DESKTOP_SERVICE)
        .await
        .unwrap();
    let connection = PortalConnection::new(bus.connect().await).await.unwrap();
    let session = keyboard(&connection);
    original
        .connection
        .release_name(PORTAL_DESKTOP_SERVICE)
        .await
        .unwrap();
    original
        .connection
        .request_name(PORTAL_DESKTOP_SERVICE)
        .await
        .unwrap();
    wait_until(|| !session.is_valid()).await;
    assert!(connection.ensure_current().await.is_err());
    assert!(type_text_with_keysyms(&session, &[97]).await.is_err());
    assert!(original.calls().is_empty());
    let fresh = PortalConnection::new(bus.connect().await).await.unwrap();
    type_text_with_keysyms(&keyboard(&fresh), &[97])
        .await
        .unwrap();
}
#[tokio::test]
async fn portal_owner_response_streams_reject_other_senders_on_both_paths() {
    tokio::time::timeout(Duration::from_secs(10),async {
        let bus=TestBus::start(); let original=Portal::start(&bus).await;
        original.connection.request_name(PORTAL_DESKTOP_SERVICE).await.unwrap();
        let attacker=bus.connect().await;
        let connection=PortalConnection::new(bus.connect().await).await.unwrap();
        for returned in [false,true] {
            let (predicted,mut stream)=portal_request_stream(&connection,"owner_test").await.unwrap();
            let handle=OwnedObjectPath::try_from(if returned { format!("{predicted}_returned") } else { predicted.clone() }).unwrap();
            let send=async { loop {
                let results:HashMap<String,OwnedVariant>=HashMap::new();
                attacker.emit_signal(None::<&str>,handle.as_str(),PORTAL_REQUEST_INTERFACE,"Response",&(7_u32,&results)).await.unwrap();
                original.connection.emit_signal(None::<&str>,handle.as_str(),PORTAL_REQUEST_INTERFACE,"Response",&(0_u32,results)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            } };
            let receive=await_portal_response(&connection,handle.clone(),&predicted,&mut stream);
            tokio::select! { response=receive => assert_eq!(response.unwrap().0,0), ()=send => unreachable!() }
        }
    }).await.unwrap();
}
#[tokio::test]
async fn portal_owner_cleanup_guards_keep_the_original_destination() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let bus = TestBus::start();
        let original = Portal::start(&bus).await;
        original
            .connection
            .request_name(PORTAL_DESKTOP_SERVICE)
            .await
            .unwrap();
        let replacement = Portal::start(&bus).await;
        let connection = PortalConnection::new(bus.connect().await).await.unwrap();
        let keyboard = keyboard(&connection);
        let pointer = pointer(&connection);
        let mut keys = KeyboardReleaseGuard::new(
            &keyboard,
            Arc::clone(&keyboard.input_lock).lock_owned().await,
        );
        keys.push(PortalKey::Keysym(97));
        keys.push(PortalKey::Keycode(29));
        let mut button =
            PointerReleaseGuard::new(&pointer, Arc::clone(&pointer.input_lock).lock_owned().await);
        button.arm(BTN_LEFT);
        let cleanup =
            PortalSessionCleanup::new(connection.clone(), keyboard.session_handle.clone());
        original
            .connection
            .release_name(PORTAL_DESKTOP_SERVICE)
            .await
            .unwrap();
        replacement
            .connection
            .request_name(PORTAL_DESKTOP_SERVICE)
            .await
            .unwrap();
        wait_until(|| !connection.is_current()).await;
        drop(keys);
        drop(button);
        drop(cleanup);
        wait_until(|| original.calls().iter().filter(|m| *m == "Close").count() == 3).await;
        for expected in [
            "NotifyKeyboardKeysym",
            "NotifyKeyboardKeycode",
            "NotifyPointerButton",
        ] {
            assert!(original.calls().iter().any(|m| m == expected));
        }
        wait_until(|| {
            keyboard.input_lock.try_lock().is_ok() && pointer.input_lock.try_lock().is_ok()
        })
        .await;
        assert!(replacement.calls().is_empty());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn portal_owner_transient_lookup_failure_preserves_the_capability() {
    let current = AtomicBool::new(true);
    let expected = OwnedUniqueName::try_from(":1.42").unwrap();
    let timeout = tokio::time::timeout(
        Duration::ZERO,
        std::future::pending::<zbus::fdo::Result<OwnedUniqueName>>(),
    )
    .await;
    assert!(validate_owner_observation(&current, &expected, timeout).is_err());
    assert!(current.load(Ordering::Acquire));
    assert!(validate_owner_observation(
        &current,
        &expected,
        Ok(Err(zbus::fdo::Error::Failed(
            "temporary lookup failure".into()
        )))
    )
    .is_err());
    assert!(current.load(Ordering::Acquire));
    assert!(validate_owner_observation(&current, &expected, Ok(Ok(expected.clone()))).is_ok());
    assert!(current.load(Ordering::Acquire));
}

#[test]
fn portal_owner_confirmed_loss_permanently_revokes_the_capability() {
    let expected = OwnedUniqueName::try_from(":1.42").unwrap();
    for observation in [
        Ok(OwnedUniqueName::try_from(":1.43").unwrap()),
        Err(zbus::fdo::Error::NameHasNoOwner("no portal owner".into())),
    ] {
        let current = AtomicBool::new(true);
        assert!(validate_owner_observation(&current, &expected, Ok(observation)).is_err());
        assert!(!current.load(Ordering::Acquire));
        // Even a matching later observation never resets a revoked capability.
        let _ = validate_owner_observation(&current, &expected, Ok(Ok(expected.clone())));
        assert!(!current.load(Ordering::Acquire));
    }
}

struct MockTypingPortal(
    tokio::sync::mpsc::UnboundedSender<(u32, tokio::sync::oneshot::Sender<()>)>,
);
#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop")]
impl MockTypingPortal {
    async fn notify_keyboard_keysym(
        &self,
        _session: OwnedObjectPath,
        _options: HashMap<String, OwnedVariant>,
        _keysym: i32,
        state: u32,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.0.send((state, tx)).unwrap();
        let _ = rx.await;
    }
}
struct MockTypingSession(
    tokio::sync::mpsc::UnboundedSender<(u32, tokio::sync::oneshot::Sender<()>)>,
);
#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl MockTypingSession {
    async fn close(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.0.send((2, tx)).unwrap();
        let _ = rx.await;
    }
}
#[tokio::test]
async fn typing_cancellation_releases_keys_and_closes_before_idle() {
    let bus = TestBus::start();
    let service = bus.connect().await;
    let client = bus.connect().await;
    let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let path =
        OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/test/typing").unwrap();
    service
        .object_server()
        .at(PORTAL_DESKTOP_PATH, MockTypingPortal(tx.clone()))
        .await
        .unwrap();
    service
        .object_server()
        .at(path.clone(), MockTypingSession(tx))
        .await
        .unwrap();
    service.request_name(PORTAL_DESKTOP_SERVICE).await.unwrap();
    let session = PortalKeyboardSession {
        connection: PortalConnection::new(client).await.unwrap(),
        session_handle: path,
        input_lock: Arc::new(AsyncMutex::new(())),
        valid: Arc::new(AtomicBool::new(true)),
    };
    let operation_session = session.clone();
    let task = tokio::spawn(async move {
        type_text_with_keysyms(&operation_session, &[xkeysym::key::a as i32]).await
    });
    let (state, reply) = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state, KEY_PRESSED);
    task.abort();
    let _ = task.await;
    reply.send(()).unwrap();
    assert!(!session.is_valid());
    let mut idle = std::pin::pin!(session.wait_for_input_idle());
    for expected in [KEY_RELEASED, 2] {
        let (state, reply) = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, expected);
        assert!(futures_util::poll!(idle.as_mut()).is_pending());
        reply.send(()).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), idle)
        .await
        .unwrap();
    assert!(session.input_lock.try_lock().is_ok());
}
