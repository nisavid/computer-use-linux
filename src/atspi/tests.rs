//! Protocol fixtures deliberately use primitive wire values, not the binding's
//! response types. Signatures follow the AT-SPI Accessible, Action, Component,
//! Text, Value, EditableText and Registry XML interfaces.
use super::*;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
use zbus::{wire::serialized::Context, Message, Variant};

fn decode_wire<T: serde::de::DeserializeOwned + Type>(
    signature: &str,
    value: &impl Serialize,
) -> zbus::wire::Result<T> {
    let bytes =
        zbus::wire::to_bytes_for_signature(Context::new(zbus::wire::LE, 0), signature, value)?;
    bytes.deserialize().map(|(value, _)| value)
}

#[test]
fn object_reference_wire_accepts_null_and_rejects_malformed_owners_and_paths() {
    assert_eq!(ObjectRefOwned::SIGNATURE, "(so)");
    for owner in ["", ":1.42"] {
        let reference: ObjectRefOwned =
            decode_wire("(so)", &(owner, "/org/a11y/atspi/null")).unwrap();
        assert_eq!(reference.name_as_str(), None);
        assert_eq!(reference.path_as_str(), "/org/a11y/atspi/null");
    }
    let reference: ObjectRefOwned = decode_wire("(so)", &(":1.42", "/org/example/child")).unwrap();
    assert_eq!(reference.name_as_str(), Some(":1.42"));
    assert_eq!(reference.path_as_str(), "/org/example/child");
    for owner in ["", "org.example.Provider", "not a bus name"] {
        assert!(decode_wire::<ObjectRefOwned>("(so)", &(owner, "/valid/path")).is_err());
    }
    assert!(decode_wire::<ObjectRefOwned>("(so)", &(":1.42", "invalid/path")).is_err());
}

#[test]
fn state_wire_requires_two_words_and_preserves_low_then_high_bit_order() {
    assert_eq!(StateSet::SIGNATURE, "au");
    let states: StateSet = decode_wire("au", &vec![1_u32 << 12, 1_u32 << 11]).unwrap();
    assert!(states.focused());
    assert_eq!(states.labels(), ["focused", "read-only"]);
    let states: StateSet = decode_wire("au", &vec![0_u32, 1]).unwrap();
    assert!(!states.focused());
    assert_eq!(states.labels(), ["indeterminate"]);
    assert!(decode_wire::<StateSet>("au", &vec![0_u32, 0])
        .unwrap()
        .labels()
        .is_empty());
    for words in [vec![], vec![0_u32], vec![0, 0, 0], vec![0, 1 << 12]] {
        assert!(decode_wire::<StateSet>("au", &words).is_err());
    }
}

#[test]
fn terminal_role_and_action_records_decode_from_protocol_primitives() {
    assert_eq!(Role::SIGNATURE, "u");
    assert_eq!(decode_wire::<Role>("u", &60_u32).unwrap(), Role::Terminal);
    assert!(decode_wire::<Role>("u", &u32::MAX).is_err());
    assert_eq!(<Vec<Action>>::SIGNATURE, "a(sss)");
    let actions: Vec<Action> = decode_wire(
        "a(sss)",
        &vec![
            ("click", "Press the button", "<Control>Return"),
            ("open", "Open menu", ""),
        ],
    )
    .unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].name, "click");
    assert_eq!(actions[0].description, "Press the button");
    assert_eq!(actions[0].keybinding, "<Control>Return");
    assert_eq!(actions[1].name, "open");
}

#[test]
fn protocol_method_replies_decode_structs_and_separate_return_arguments() {
    let call = Message::method_call("/org/example/object", "Fixture")
        .sender(":1.42")
        .build(&())
        .unwrap();
    let reference = reply(
        &call,
        &((":1.42", ObjectPath::try_from("/org/example/child").unwrap()),),
    );
    let reference: ObjectRefOwned = reference.body().deserialize().unwrap();
    assert_eq!(reference.name_as_str(), Some(":1.42"));
    assert_eq!(reference.path_as_str(), "/org/example/child");
    let extents = reply(&call, &((-10_i32, 20_i32, 640_i32, 480_i32),));
    assert_eq!(
        extents
            .body()
            .deserialize::<(i32, i32, i32, i32)>()
            .unwrap(),
        (-10, 20, 640, 480)
    );
    let selection = reply(&call, &(1_i32, 4_i32));
    assert_eq!(
        selection.body().deserialize::<(i32, i32)>().unwrap(),
        (1, 4)
    );
}

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
            .expect("AT-SPI protocol tests require a private dbus-daemon; install dbus-daemon");
        let mut address = String::new();
        BufReader::new(child.stdout.take().expect("private bus stdout"))
            .read_line(&mut address)
            .expect("read private bus address");
        if address.trim().is_empty() {
            let output = child
                .wait_with_output()
                .expect("collect failed private bus startup");
            panic!(
                "private dbus-daemon failed: {}: {}",
                output.status,
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
            .expect("connect to private AT-SPI test bus")
    }
}

impl Drop for TestBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct ProviderState {
    path: String,
    name: String,
    description: String,
    children: Vec<(String, String)>,
    interfaces: Vec<String>,
    interfaces_fail: bool,
    current_value: f64,
    text: String,
    actions: Vec<i32>,
    text_requests: Vec<(i32, i32)>,
    coordinates: Vec<u32>,
    events: Vec<(String, String)>,
}

impl ProviderState {
    fn new(path: &str, name: &str) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            description: "initial description".into(),
            children: Vec::new(),
            interfaces: [
                "Accessible",
                "Action",
                "Component",
                "Text",
                "Value",
                "EditableText",
            ]
            .map(|name| format!("org.a11y.atspi.{name}"))
            .into(),
            interfaces_fail: false,
            current_value: 3.5,
            text: "hello world".into(),
            actions: Vec::new(),
            text_requests: Vec::new(),
            coordinates: Vec::new(),
            events: Vec::new(),
        }
    }

    fn properties(&self, interface: &str) -> HashMap<&'static str, Variant<'static>> {
        match interface {
            "org.a11y.atspi.Accessible" => HashMap::from([
                ("Name", Variant::from(self.name.clone())),
                ("Description", Variant::from(self.description.clone())),
                ("ChildCount", Variant::from(self.children.len() as i32)),
            ]),
            "org.a11y.atspi.Text" => HashMap::from([
                (
                    "CharacterCount",
                    Variant::from(self.text.chars().count() as i32),
                ),
                ("CaretOffset", Variant::from(3_i32)),
            ]),
            "org.a11y.atspi.Value" => HashMap::from([
                ("CurrentValue", Variant::from(self.current_value)),
                ("MinimumValue", Variant::from(-5.0_f64)),
                ("MaximumValue", Variant::from(100.0_f64)),
                ("MinimumIncrement", Variant::from(0.25_f64)),
                (
                    "Text",
                    Variant::from(format!("{} units", self.current_value)),
                ),
            ]),
            _ => HashMap::new(),
        }
    }

    fn answer(&mut self, call: &Message) -> Message {
        let header = call.header();
        let interface = header.interface().unwrap().as_str();
        let member = header.member().unwrap().as_str();
        let path = header.path().unwrap().as_str();
        if interface == "org.a11y.atspi.Registry" {
            assert_eq!(path, "/org/a11y/atspi/registry");
            assert!(matches!(member, "RegisterEvent" | "DeregisterEvent"));
            let event: String = call.body().deserialize().unwrap();
            self.events.push((member.into(), event));
            return reply(call, &());
        }
        if path != self.path {
            return error_reply(call, "org.freedesktop.DBus.Error.UnknownObject");
        }
        match (interface, member) {
            ("org.a11y.atspi.Accessible", "GetChildAtIndex") => {
                let index: i32 = call.body().deserialize().unwrap();
                let (owner, path) = self
                    .children
                    .get(index as usize)
                    .cloned()
                    .unwrap_or_else(|| (String::new(), "/org/a11y/atspi/null".into()));
                // One (so) output, not two top-level outputs.
                reply(call, &((owner, ObjectPath::try_from(path).unwrap()),))
            }
            ("org.a11y.atspi.Accessible", "GetInterfaces") if self.interfaces_fail => {
                error_reply(call, "org.freedesktop.DBus.Error.Failed")
            }
            ("org.a11y.atspi.Accessible", "GetInterfaces") => reply(call, &self.interfaces),
            ("org.a11y.atspi.Accessible", "GetRole") => reply(call, &60_u32),
            ("org.a11y.atspi.Accessible", "GetRoleName") => reply(call, &"terminal"),
            ("org.a11y.atspi.Accessible", "GetState") => {
                reply(call, &vec![1_u32 << 12, 1_u32 << 11])
            }
            ("org.a11y.atspi.Action", "GetActions") => {
                reply(call, &vec![("click", "Press", "Return")])
            }
            ("org.a11y.atspi.Action", "DoAction") => {
                let index: i32 = call.body().deserialize().unwrap();
                self.actions.push(index);
                reply(call, &(index == 0))
            }
            ("org.a11y.atspi.Component", "GetExtents") => {
                self.coordinates.push(call.body().deserialize().unwrap());
                // Component.xml specifies one struct, unlike Text.GetSelection.
                reply(call, &((-10_i32, 20_i32, 640_i32, 480_i32),))
            }
            ("org.a11y.atspi.Text", "GetText") => {
                let offsets: (i32, i32) = call.body().deserialize().unwrap();
                self.text_requests.push(offsets);
                let end = if offsets.1 == -1 {
                    self.text.chars().count()
                } else {
                    offsets.1 as usize
                };
                let text: String = self
                    .text
                    .chars()
                    .skip(offsets.0 as usize)
                    .take(end - offsets.0 as usize)
                    .collect();
                reply(call, &text)
            }
            ("org.a11y.atspi.Text", "GetNSelections") => reply(call, &1_i32),
            ("org.a11y.atspi.Text", "GetSelection") => {
                assert_eq!(call.body().deserialize::<i32>().unwrap(), 0);
                reply(call, &(1_i32, 4_i32))
            }
            ("org.a11y.atspi.EditableText", "SetTextContents") => {
                self.text = call.body().deserialize().unwrap();
                reply(call, &true)
            }
            ("org.freedesktop.DBus.Properties", "Get") => {
                let (interface, property): (String, String) = call.body().deserialize().unwrap();
                match self.properties(&interface).remove(property.as_str()) {
                    Some(value) => reply(call, &value),
                    None => error_reply(call, "org.freedesktop.DBus.Error.UnknownProperty"),
                }
            }
            ("org.freedesktop.DBus.Properties", "GetAll") => {
                let interface: String = call.body().deserialize().unwrap();
                reply(call, &self.properties(&interface))
            }
            ("org.freedesktop.DBus.Properties", "Set") => {
                let body = call.body();
                let (interface, property, value): (String, String, Variant<'_>) =
                    body.deserialize().unwrap();
                assert_eq!(interface, "org.a11y.atspi.Value");
                assert_eq!(property, "CurrentValue");
                self.current_value = f64::try_from(value).unwrap();
                reply(call, &())
            }
            _ => error_reply(call, "org.freedesktop.DBus.Error.UnknownMethod"),
        }
    }
}

fn reply(call: &Message, body: &(impl Serialize + zbus::DynamicType)) -> Message {
    Message::method_return(&call.header()).build(body).unwrap()
}

fn error_reply(call: &Message, error: &str) -> Message {
    Message::error(&call.header(), error)
        .build(&"fixture error")
        .unwrap()
}

struct Provider {
    connection: Connection,
    state: Arc<Mutex<ProviderState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Provider {
    async fn start(bus: &TestBus, path: &str, name: &str) -> Self {
        let connection = bus.connect().await;
        let state = Arc::new(Mutex::new(ProviderState::new(path, name)));
        let mut incoming = MessageStream::from(&connection);
        let server = connection.clone();
        let server_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            while let Some(message) = incoming.next().await {
                let message = message.expect("read mock provider request");
                if message.message_type() != zbus::message::Type::MethodCall {
                    continue;
                }
                let response = server_state.lock().unwrap().answer(&message);
                server
                    .send(&response)
                    .await
                    .expect("send mock provider response");
            }
        });
        Self {
            connection,
            state,
            task,
        }
    }

    fn owner(&self) -> String {
        self.connection.unique_name().unwrap().to_string()
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn private_bus_routes_references_and_reads_live_properties() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let bus = TestBus::start();
        let registry = Provider::start(&bus, "/org/a11y/atspi/accessible/root", "registry").await;
        registry
            .connection
            .request_name("org.a11y.atspi.Registry")
            .await
            .unwrap();
        let child = Provider::start(&bus, "/org/example/child", "child provider").await;
        registry
            .state
            .lock()
            .unwrap()
            .children
            .push((child.owner(), "/org/example/child".into()));
        let client = AccessibilityConnection::from_address(&bus.address)
            .await
            .unwrap();
        let root = client.root_accessible_on_registry().await.unwrap();
        assert_eq!(root.name().await.unwrap(), "registry");
        assert_eq!(root.child_count().await.unwrap(), 1);
        let reference = root.get_child_at_index(0).await.unwrap();
        assert_eq!(reference.name_as_str(), Some(child.owner().as_str()));
        assert_eq!(reference.path_as_str(), "/org/example/child");
        let accessible = reference
            .as_accessible_proxy(client.connection())
            .await
            .unwrap();
        assert_eq!(accessible.name().await.unwrap(), "child provider");
        assert_eq!(
            accessible.description().await.unwrap(),
            "initial description"
        );
        assert_eq!(accessible.get_role().await.unwrap(), Role::Terminal);
        assert_eq!(accessible.get_role_name().await.unwrap(), "terminal");
        assert!(accessible.get_state().await.unwrap().focused());
        {
            let mut state = child.state.lock().unwrap();
            state.name = "renamed".into();
            state.description = "changed without PropertiesChanged".into();
            state
                .children
                .push((registry.owner(), "/org/a11y/atspi/accessible/root".into()));
        }
        assert_eq!(accessible.name().await.unwrap(), "renamed");
        assert_eq!(
            accessible.description().await.unwrap(),
            "changed without PropertiesChanged"
        );
        assert_eq!(accessible.child_count().await.unwrap(), 1);
        let parent = accessible
            .get_child_at_index(0)
            .await
            .unwrap()
            .as_accessible_proxy(client.connection())
            .await
            .unwrap();
        assert_eq!(parent.name().await.unwrap(), "registry");
        let null = root.get_child_at_index(99).await.unwrap();
        assert!(null.as_accessible_proxy(client.connection()).await.is_err());
        registry.state.lock().unwrap().children[0].0 = "org.example.WellKnownButNotUnique".into();
        assert!(root.get_child_at_index(0).await.is_err());
    })
    .await
    .expect("private-bus AT-SPI test timed out");
}

#[tokio::test]
async fn private_bus_optional_interfaces_use_exact_names_and_propagate_errors() {
    tokio::time::timeout(Duration::from_secs(10), async {
    let bus = TestBus::start();
    let provider = Provider::start(&bus, "/org/example/object", "provider").await;
    let client = bus.connect().await;
    let accessible = ObjectRefOwned::new(
        provider.connection.unique_name().unwrap().to_owned().into(),
        ObjectPath::try_from("/org/example/object").unwrap(),
    )
    .as_accessible_proxy(&client)
    .await
    .unwrap();
    provider.state.lock().unwrap().interfaces = vec![
        "Action".into(),
        "org.a11y.atspi.action".into(),
        "org.a11y.atspi.TextExtra".into(),
    ];
    let proxies = accessible.proxies().await.unwrap();
    assert!(proxies.action().await.is_err());
    assert!(proxies.component().await.is_err());
    assert!(proxies.value().await.is_err());
    assert!(proxies.text().await.is_err());
    assert!(proxies.editable_text().await.is_err());
    provider.state.lock().unwrap().interfaces = vec!["org.a11y.atspi.Action".into()];
    assert!(accessible.proxies().await.unwrap().action().await.is_ok());
    provider.state.lock().unwrap().interfaces_fail = true;
    let error = match accessible.proxies().await {
        Ok(_) => panic!("GetInterfaces error lost"),
        Err(error) => error,
    };
    assert!(
        matches!(error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.Failed")
    );
    })
    .await
    .expect("private-bus AT-SPI test timed out");
}

#[tokio::test]
async fn private_bus_action_value_text_and_component_interoperate() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let bus = TestBus::start();
        let provider = Provider::start(&bus, "/org/example/controls", "provider").await;
        let client = bus.connect().await;
        let accessible = ObjectRefOwned::new(
            provider.connection.unique_name().unwrap().to_owned().into(),
            ObjectPath::try_from("/org/example/controls").unwrap(),
        )
        .as_accessible_proxy(&client)
        .await
        .unwrap();
        let proxies = accessible.proxies().await.unwrap();
        let actions = proxies.action().await.unwrap();
        let records = actions.get_actions().await.unwrap();
        assert_eq!(
            (
                &records[0].name,
                &records[0].description,
                &records[0].keybinding
            ),
            (
                &"click".to_owned(),
                &"Press".to_owned(),
                &"Return".to_owned()
            )
        );
        assert!(actions.do_action(0).await.unwrap());
        assert!(!actions.do_action(7).await.unwrap());
        assert_eq!(provider.state.lock().unwrap().actions, [0, 7]);
        let value = proxies.value().await.unwrap();
        assert_eq!(value.current_value().await.unwrap(), 3.5);
        provider.state.lock().unwrap().current_value = 4.5;
        assert_eq!(value.current_value().await.unwrap(), 4.5);
        assert_eq!(value.minimum_value().await.unwrap(), -5.0);
        assert_eq!(value.maximum_value().await.unwrap(), 100.0);
        assert_eq!(value.minimum_increment().await.unwrap(), 0.25);
        value.set_current_value(42.25).await.unwrap();
        assert_eq!(provider.state.lock().unwrap().current_value, 42.25);
        assert_eq!(value.current_value().await.unwrap(), 42.25);
        assert_eq!(value.text().await.unwrap(), "42.25 units");
        let text = proxies.text().await.unwrap();
        assert_eq!(text.get_text(0, -1).await.unwrap(), "hello world");
        assert_eq!(text.get_text(1, 4).await.unwrap(), "ell");
        assert_eq!(text.character_count().await.unwrap(), 11);
        assert_eq!(text.caret_offset().await.unwrap(), 3);
        assert_eq!(text.get_nselections().await.unwrap(), 1);
        assert_eq!(text.get_selection(0).await.unwrap(), (1, 4));
        assert!(proxies
            .editable_text()
            .await
            .unwrap()
            .set_text_contents("hëllo 🌍")
            .await
            .unwrap());
        assert_eq!(text.get_text(0, -1).await.unwrap(), "hëllo 🌍");
        assert_eq!(text.character_count().await.unwrap(), 7);
        assert_eq!(
            provider.state.lock().unwrap().text_requests,
            [(0, -1), (1, 4), (0, -1)]
        );
        assert_eq!(
            proxies
                .component()
                .await
                .unwrap()
                .get_extents(0)
                .await
                .unwrap(),
            (-10, 20, 640, 480)
        );
        assert_eq!(provider.state.lock().unwrap().coordinates, [0]);
    })
    .await
    .expect("private-bus AT-SPI test timed out");
}

#[tokio::test]
async fn private_bus_activation_listener_registers_lowercase_and_removes_match() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let bus = TestBus::start();
        let registry = Provider::start(&bus, "/org/a11y/atspi/accessible/root", "registry").await;
        registry
            .connection
            .request_name("org.a11y.atspi.Registry")
            .await
            .unwrap();
        let client = AccessibilityConnection::from_address(&bus.address)
            .await
            .unwrap();
        let mut events = Box::pin(client.event_stream());
        client.register_activation_listener().await.unwrap();
        assert_eq!(
            registry.state.lock().unwrap().events,
            [("RegisterEvent".into(), "window:activate".into())]
        );
        registry
            .connection
            .emit_signal(
                None::<&str>,
                "/org/example/window",
                "org.a11y.atspi.Event.Window",
                "Deactivate",
                &(),
            )
            .await
            .unwrap();
        registry
            .connection
            .emit_signal(
                None::<&str>,
                "/org/example/window",
                "org.a11y.atspi.Event.Window",
                "Activate",
                &(),
            )
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(3), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(event.header().member().unwrap().as_str(), "Activate");
        client.deregister_activation_listener().await.unwrap();
        assert_eq!(
            registry.state.lock().unwrap().events,
            [
                ("RegisterEvent".into(), "window:activate".into()),
                ("DeregisterEvent".into(), "window:activate".into())
            ]
        );
        registry
            .connection
            .emit_signal(
                None::<&str>,
                "/org/example/window",
                "org.a11y.atspi.Event.Window",
                "Activate",
                &(),
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), events.next())
                .await
                .is_err()
        );
        let removal = DBusProxy::new(client.connection())
            .await
            .unwrap()
            .remove_match_rule(
                MatchRule::try_from(
                    "type='signal',interface='org.a11y.atspi.Event.Window',member='Activate'",
                )
                .unwrap(),
            )
            .await;
        assert!(
            removal.is_err(),
            "listener deregistration left its bus match installed"
        );
    })
    .await
    .expect("private-bus AT-SPI test timed out");
}
