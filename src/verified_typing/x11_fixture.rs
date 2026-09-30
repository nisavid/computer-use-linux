//! Public-observer wire tests inside a disposable filesystem/process namespace.
//! The constructed server and event nodes exercise logic, never native input.
use super::*;
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    process::Command,
    sync::atomic::{AtomicBool, AtomicU16, Ordering},
};

type WireClient = (Arc<Mutex<UnixStream>>, Arc<AtomicU16>, Arc<AtomicBool>);

#[test]
#[ignore = "requires bubblewrap user namespaces; uses only constructed X11/sysfs"]
fn private_wire_observer_contract() {
    const CHILD: &str = "CUL_TEST_OBSERVER_WIRE_CHILD";
    if let Ok(mode) = std::env::var(CHILD) {
        run_child(&mode);
        return;
    }
    let directory = std::env::temp_dir().join(format!("cul-observer-wire-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("sysfs/input900/event900")).unwrap();
    std::fs::write(directory.join("sysfs/input900/event900/dev"), "1:3\n").unwrap();
    let executable = directory.join("Xorg");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    for mode in [
        "flood",
        "slave-none",
        "slave-pointer-root",
        "slave-focus-transient",
        "user-time",
        "owner-property",
        "unknown-property",
        "user-time-window-property",
        "deleted-user-time",
        "synthetic-user-time",
        "foreign-window-user-time",
        "iso-level",
        "good",
        "repeat",
        "mismatch",
        "dead",
        "compose",
        "key-name",
        "action",
        "type",
        "locked",
        "duplicate-owner",
        "shift-symbol",
        "libinput-disabled",
        "target-pid",
        "target-window",
        "foreign-source",
        "duplicate-stroke",
        "unobserved",
        "input-after-completion",
    ] {
        let output = Command::new("bwrap")
            .args([
                "--unshare-all",
                "--die-with-parent",
                "--new-session",
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--dir",
                "/dev/input",
                "--ro-bind",
                "/dev/null",
                "/dev/input/event900",
                "--ro-bind",
                "/dev/zero",
                "/dev/input/event901",
                "--ro-bind",
            ])
            .arg(directory.join("sysfs"))
            .args([
                "/sys/class/input",
                "--tmpfs",
                "/tmp",
                "--dir",
                "/tmp/.X11-unix",
                "--ro-bind",
            ])
            .arg(&directory)
            .args([
                "/tmp/fixture",
                "--clearenv",
                "--setenv",
                "PATH",
                "/usr/bin:/bin",
                "--setenv",
                CHILD,
                mode,
                "/tmp/fixture/Xorg",
                "--exact",
            ])
            .arg(format!(
                "{}::private_wire_observer_contract",
                module_path!().split_once("::").unwrap().1
            ))
            .args(["--ignored", "--nocapture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "mode {mode}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

fn run_child(mode: &str) {
    let listener = UnixListener::bind("/tmp/.X11-unix/X65000").unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let mode_owned = mode.to_owned();
    let worker = std::thread::spawn(move || serve(listener, &mode_owned, sender));
    let stroke = Stroke {
        keycode: 30,
        left_shift: true,
    };
    let prepared = Observer::prepare("input900", &[stroke], "A", ":65000");
    match mode {
        "flood" => match prepared {
            Err(QualificationFailure::Unknown(detail)) => {
                assert!(detail.contains("byte budget"), "{detail}")
            }
            _ => panic!("continuous X11 bytes did not exhaust the bounded observation budget"),
        },
        "repeat" | "dead" | "compose" | "key-name" | "action" | "type" | "locked"
        | "duplicate-owner" | "libinput-disabled" | "shift-symbol" => {
            assert!(
                matches!(prepared, Err(QualificationFailure::Unknown(_))),
                "mode {mode} did not refuse Unknown"
            );
        }
        "mismatch" => assert!(matches!(
            prepared,
            Err(QualificationFailure::Incompatible(_))
        )),
        _ => {
            let mut observer = prepared.unwrap_or_else(|error| panic!("mode {mode}: {error:?}"));
            let binding = observer.recheck_after_focus(
                Some(if mode == "target-pid" { 4243 } else { 4242 }),
                Some(if mode == "target-window" { 4 } else { 2 }),
            );
            if matches!(mode, "target-pid" | "target-window" | "slave-pointer-root") {
                assert!(matches!(binding, Err(QualificationFailure::Unknown(_))));
            } else {
                binding.unwrap();
                if mode == "slave-focus-transient" {
                    let (writer, sequence, selected) = receiver.recv().unwrap();
                    // Sibling A→B→A does not notify their common root. The
                    // constructed server emits only the A-window events that
                    // this client actually selected for the slave device.
                    if selected.load(Ordering::SeqCst) {
                        for event_type in [10, 9] {
                            let mut event = xinput::EnterEvent {
                                response_type: 35,
                                extension: 134,
                                sequence: sequence.load(Ordering::SeqCst),
                                event_type,
                                deviceid: 12,
                                sourceid: 12,
                                root: 1,
                                event: 2,
                                ..Default::default()
                            }
                            .serialize();
                            let length = ((event.len() - 32) / 4) as u32;
                            event[4..8].copy_from_slice(&length.to_le_bytes());
                            writer.lock().unwrap().write_all(&event).unwrap();
                        }
                    }
                    assert!(
                        matches!(
                            observer.before_stroke(stroke),
                            Err(QualificationFailure::Unknown(_))
                        ),
                        "transient foreign slave focus still authorized the stroke"
                    );
                    drop(observer);
                    worker.join().unwrap();
                    return;
                }
                observer.before_stroke(stroke).unwrap();
                let (writer, sequence, _) = receiver.recv().unwrap();
                if mode != "unobserved" {
                    for (index, (detail, press)) in
                        [(50, true), (38, true), (38, false), (50, false)]
                            .into_iter()
                            .enumerate()
                    {
                        let mut event = xinput::RawKeyPressEvent {
                            response_type: 35,
                            extension: 134,
                            sequence: sequence.load(Ordering::SeqCst),
                            length: 0,
                            event_type: if press { 13 } else { 14 },
                            deviceid: 3,
                            sourceid: if mode == "foreign-source" { 13 } else { 12 },
                            detail,
                            ..Default::default()
                        }
                        .serialize();
                        let length = ((event.len() - 32) / 4) as u32;
                        event[4..8].copy_from_slice(&length.to_le_bytes());
                        writer.lock().unwrap().write_all(&event).unwrap();
                        if mode == "duplicate-stroke" && index == 1 {
                            writer.lock().unwrap().write_all(&event).unwrap();
                        }
                    }
                }
                if matches!(
                    mode,
                    "user-time"
                        | "owner-property"
                        | "unknown-property"
                        | "user-time-window-property"
                        | "deleted-user-time"
                        | "synthetic-user-time"
                        | "foreign-window-user-time"
                ) {
                    let event = xproto::PropertyNotifyEvent {
                        response_type: 28
                            | if mode == "synthetic-user-time" {
                                0x80
                            } else {
                                0
                            },
                        sequence: sequence.load(Ordering::SeqCst),
                        window: if mode == "foreign-window-user-time" {
                            4
                        } else {
                            2
                        },
                        atom: match mode {
                            "owner-property" => 102,
                            "unknown-property" => 104,
                            "user-time-window-property" => 105,
                            _ => 103,
                        },
                        time: 100,
                        state: if mode == "deleted-user-time" {
                            xproto::Property::DELETE
                        } else {
                            xproto::Property::NEW_VALUE
                        },
                    };
                    // Core X11 events occupy 32 bytes on the wire even when
                    // their generated Serialize representation is shorter.
                    let event: [u8; 32] = (&event).into();
                    writer.lock().unwrap().write_all(&event).unwrap();
                }
                let result = observer.after_stroke(stroke);
                if mode == "good"
                    || mode == "iso-level"
                    || mode == "slave-none"
                    || mode == "user-time"
                    || mode == "input-after-completion"
                {
                    result.unwrap();
                    observer.poll_during_stroke(stroke).unwrap();
                    if mode == "input-after-completion" {
                        let event = xinput::RawKeyPressEvent {
                            response_type: 35,
                            extension: 134,
                            sequence: sequence.load(Ordering::SeqCst),
                            event_type: 13,
                            deviceid: 3,
                            sourceid: 12,
                            detail: 38,
                            ..Default::default()
                        }
                        .serialize();
                        writer.lock().unwrap().write_all(&event).unwrap();
                        assert!(matches!(
                            observer.finish_observation(),
                            Err(QualificationFailure::Unknown(_))
                        ));
                    } else {
                        observer.finish_observation().unwrap();
                    }
                } else {
                    match result {
                        Err(QualificationFailure::Unknown(detail)) => {
                            if matches!(
                                mode,
                                "owner-property"
                                    | "unknown-property"
                                    | "user-time-window-property"
                                    | "deleted-user-time"
                                    | "synthetic-user-time"
                                    | "foreign-window-user-time"
                            ) {
                                assert!(
                                    detail.contains("changed during qualification"),
                                    "{mode}: {detail}"
                                );
                            }
                        }
                        _ => panic!("{mode} did not stop"),
                    }
                }
            }
        }
    }
    worker.join().unwrap();
}

fn packet(mut bytes: Vec<u8>, sequence: u16) -> Vec<u8> {
    bytes.resize(bytes.len().max(32).next_multiple_of(4), 0);
    bytes[2..4].copy_from_slice(&sequence.to_le_bytes());
    let length = ((bytes.len() - 32) / 4) as u32;
    bytes[4..8].copy_from_slice(&length.to_le_bytes());
    bytes
}
fn serve(listener: UnixListener, mode: &str, sender: std::sync::mpsc::Sender<WireClient>) {
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = stream.try_clone().unwrap();
    let writer = Arc::new(Mutex::new(stream));
    let sequence = Arc::new(AtomicU16::new(0));
    let slave_focus_selected = Arc::new(AtomicBool::new(false));
    sender
        .send((
            writer.clone(),
            sequence.clone(),
            slave_focus_selected.clone(),
        ))
        .unwrap();
    let mut setup_request = [0; 12];
    reader.read_exact(&mut setup_request).unwrap();
    assert_eq!(setup_request[0], b'l');
    let auth_name = u16::from_le_bytes(setup_request[6..8].try_into().unwrap()) as usize;
    let auth_data = u16::from_le_bytes(setup_request[8..10].try_into().unwrap()) as usize;
    let mut auth = vec![0; auth_name.next_multiple_of(4) + auth_data.next_multiple_of(4)];
    reader.read_exact(&mut auth).unwrap();
    let mut setup = xproto::Setup {
        status: 1,
        protocol_major_version: 11,
        resource_id_base: 0x200000,
        resource_id_mask: 0x1fffff,
        maximum_request_length: 65535,
        min_keycode: 8,
        max_keycode: 65,
        roots: vec![xproto::Screen {
            root: 1,
            width_in_pixels: 100,
            height_in_pixels: 100,
            root_depth: 24,
            ..Default::default()
        }],
        ..Default::default()
    }
    .serialize();
    let setup_length = ((setup.len() - 8) / 4) as u16;
    setup[6..8].copy_from_slice(&setup_length.to_le_bytes());
    writer.lock().unwrap().write_all(&setup).unwrap();
    if mode == "flood" {
        let mut request = [0; 4];
        reader.read_exact(&mut request).unwrap();
        let len = u16::from_le_bytes(request[2..4].try_into().unwrap()) as usize * 4;
        let mut rest = vec![0; len - 4];
        reader.read_exact(&mut rest).unwrap();
        let frame = xproto::QueryExtensionReply {
            sequence: 1,
            ..Default::default()
        }
        .serialize();
        let bytes: Vec<_> = frame.into_iter().cycle().take(4 * 1024 * 1024).collect();
        let _ = writer.lock().unwrap().write_all(&bytes);
        return;
    }
    loop {
        let mut header = [0; 4];
        if reader.read_exact(&mut header).is_err() {
            break;
        }
        let count = sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let len = u16::from_le_bytes(header[2..4].try_into().unwrap()) as usize * 4;
        assert!((4..=65536).contains(&len));
        let mut request = vec![0; len];
        request[..4].copy_from_slice(&header);
        reader.read_exact(&mut request[4..]).unwrap();
        let device = request.get(4).copied().unwrap_or(0);
        let bytes = match (header[0], header[1]) {
            (98, _) => {
                let name_len = u16::from_le_bytes(request[4..6].try_into().unwrap()) as usize;
                let name = &request[8..8 + name_len];
                let (present, major_opcode, first_event) = match name {
                    b"XKEYBOARD" => (true, 133, 64),
                    b"XInputExtension" => (true, 134, 0),
                    _ => (false, 0, 0),
                };
                Some(
                    xproto::QueryExtensionReply {
                        present,
                        major_opcode,
                        first_event,
                        ..Default::default()
                    }
                    .serialize()
                    .to_vec(),
                )
            }
            (16, _) => {
                let name_len = u16::from_le_bytes(request[4..6].try_into().unwrap()) as usize;
                let name = &request[8..8 + name_len];
                let atom = match name {
                    b"Device Node" => 100,
                    b"libinput Send Events Mode Enabled" => 101,
                    b"_NET_WM_PID" => 102,
                    _ => 103,
                };
                Some(
                    xproto::InternAtomReply {
                        atom,
                        ..Default::default()
                    }
                    .serialize()
                    .to_vec(),
                )
            }
            (43, _) => Some(
                xproto::GetInputFocusReply {
                    focus: 2,
                    ..Default::default()
                }
                .serialize()
                .to_vec(),
            ),
            (44, _) => Some(
                xproto::QueryKeymapReply {
                    ..Default::default()
                }
                .serialize()
                .to_vec(),
            ),
            (15, _) => Some(
                xproto::QueryTreeReply {
                    root: 1,
                    parent: 1,
                    children: Vec::new(),
                    sequence: 0,
                    length: 0,
                }
                .serialize(),
            ),
            (20, _) => {
                let window = u32::from_le_bytes(request[4..8].try_into().unwrap());
                Some(
                    xproto::GetPropertyReply {
                        format: if window == 2 { 32 } else { 0 },
                        type_: if window == 2 {
                            u32::from(xproto::AtomEnum::CARDINAL)
                        } else {
                            0
                        },
                        value_len: if window == 2 { 1 } else { 0 },
                        value: if window == 2 {
                            4242u32.to_le_bytes().to_vec()
                        } else {
                            Vec::new()
                        },
                        ..Default::default()
                    }
                    .serialize(),
                )
            }
            (2, _) | (133, 1) => None,
            (134, 46) => {
                let window = u32::from_le_bytes(request[4..8].try_into().unwrap());
                let count = u16::from_le_bytes(request[8..10].try_into().unwrap());
                let mut offset = 12;
                for _ in 0..count {
                    let id = u16::from_le_bytes(request[offset..offset + 2].try_into().unwrap());
                    let words =
                        u16::from_le_bytes(request[offset + 2..offset + 4].try_into().unwrap())
                            as usize;
                    let mask = if words == 0 {
                        0
                    } else {
                        u32::from_le_bytes(request[offset + 4..offset + 8].try_into().unwrap())
                    };
                    if window == 2
                        && (id == 12 || id == 0)
                        && mask & u32::from(XIEventMask::FOCUS_IN | XIEventMask::FOCUS_OUT) != 0
                    {
                        slave_focus_selected.store(true, Ordering::SeqCst);
                    }
                    offset += 4 + words * 4;
                }
                None
            }
            (133, 0) => Some(
                xkb::UseExtensionReply {
                    supported: true,
                    server_major: 1,
                    server_minor: 0,
                    ..Default::default()
                }
                .serialize()
                .to_vec(),
            ),
            (133, 8) => Some(map(device, mode).serialize()),
            (133, 17) => {
                let mut names = vec![xkb::KeyName { name: *b"XXXX" }; 58];
                names[30] = xkb::KeyName {
                    name: if mode == "key-name" {
                        *b"AC02"
                    } else {
                        *b"AC01"
                    },
                };
                names[42] = xkb::KeyName { name: *b"LFSH" };
                Some(
                    xkb::GetNamesReply {
                        device_id: device,
                        min_key_code: 8,
                        max_key_code: 65,
                        first_key: 8,
                        n_keys: 58,
                        value_list: xkb::GetNamesValueList {
                            key_names: Some(names),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                    .serialize(),
                )
            }
            (133, 6) => {
                let mut controls = xkb::GetControlsReply {
                    device_id: device,
                    num_groups: 1,
                    ..Default::default()
                };
                if mode == "repeat" {
                    controls.per_key_repeat[38 / 8] = 1 << (38 % 8);
                }
                Some(controls.serialize().to_vec())
            }
            (133, 4) => Some(
                xkb::GetStateReply {
                    device_id: device,
                    locked_mods: if mode == "locked" {
                        xproto::ModMask::LOCK
                    } else {
                        xproto::ModMask::default()
                    },
                    ..Default::default()
                }
                .serialize()
                .to_vec(),
            ),
            (134, 47) => Some(
                xinput::XIQueryVersionReply {
                    major_version: 2,
                    minor_version: 0,
                    ..Default::default()
                }
                .serialize()
                .to_vec(),
            ),
            (134, 48) => Some(
                xinput::XIQueryDeviceReply {
                    sequence: 0,
                    length: 0,
                    infos: vec![
                        xinput::XIDeviceInfo {
                            deviceid: 12,
                            type_: DeviceType::SLAVE_KEYBOARD,
                            attachment: 3,
                            enabled: true,
                            name: b"same-name".to_vec(),
                            classes: key_class(12),
                        },
                        xinput::XIDeviceInfo {
                            deviceid: 13,
                            type_: DeviceType::SLAVE_KEYBOARD,
                            attachment: 3,
                            enabled: true,
                            name: b"same-name".to_vec(),
                            classes: key_class(13),
                        },
                        xinput::XIDeviceInfo {
                            deviceid: 3,
                            type_: DeviceType::MASTER_KEYBOARD,
                            attachment: 2,
                            enabled: true,
                            name: b"master".to_vec(),
                            classes: key_class(3),
                        },
                        xinput::XIDeviceInfo {
                            deviceid: 2,
                            type_: DeviceType::MASTER_POINTER,
                            attachment: 3,
                            enabled: true,
                            name: b"pointer".to_vec(),
                            classes: Vec::new(),
                        },
                    ],
                }
                .serialize(),
            ),
            (134, 45) => Some(
                xinput::XIGetClientPointerReply {
                    set: true,
                    deviceid: 2,
                    ..Default::default()
                }
                .serialize()
                .to_vec(),
            ),
            (134, 50) => Some(
                xinput::XIGetFocusReply {
                    focus: if device == 12 && mode == "slave-none" {
                        0
                    } else if device == 12 && mode == "slave-pointer-root" {
                        1
                    } else {
                        2
                    },
                    ..Default::default()
                }
                .serialize()
                .to_vec(),
            ),
            (134, 59) => {
                let atom = u32::from_le_bytes(request[8..12].try_into().unwrap());
                let (type_, bytes) = if atom == 100 {
                    (
                        u32::from(xproto::AtomEnum::STRING),
                        if device == 12 || mode == "duplicate-owner" {
                            b"/dev/input/event900".to_vec()
                        } else {
                            b"/dev/input/event901".to_vec()
                        },
                    )
                } else {
                    (
                        u32::from(xproto::AtomEnum::INTEGER),
                        if mode == "libinput-disabled" {
                            vec![1, 0]
                        } else {
                            vec![0, 0]
                        },
                    )
                };
                Some(
                    xinput::XIGetPropertyReply {
                        type_,
                        bytes_after: 0,
                        num_items: bytes.len() as u32,
                        items: XIGetPropertyItems::Data8(bytes),
                        sequence: 0,
                        length: 0,
                    }
                    .serialize(),
                )
            }
            (134, 30) => Some(
                xinput::QueryDeviceStateReply {
                    xi_reply_type: 30,
                    sequence: 0,
                    length: 0,
                    classes: vec![xinput::InputState {
                        len: 36,
                        data: xinput::InputStateData::Key(xinput::InputStateDataKey {
                            num_keys: 58,
                            keys: [0; 32],
                        }),
                    }],
                }
                .serialize(),
            ),
            _ => panic!("unhandled X11 request {} minor {}", header[0], header[1]),
        };
        if let Some(bytes) = bytes {
            writer
                .lock()
                .unwrap()
                .write_all(&packet(bytes, count))
                .unwrap();
        }
    }
}

fn key_class(source: u16) -> Vec<xinput::DeviceClass> {
    vec![xinput::DeviceClass {
        len: 60,
        sourceid: source,
        data: xinput::DeviceClassData::Key(xinput::DeviceClassDataKey {
            keys: (8..=65).collect(),
        }),
    }]
}

fn map(device: u8, mode: &str) -> xkb::GetMapReply {
    let mut syms = vec![
        xkb::KeySymMap {
            kt_index: [0; 4],
            group_info: 1,
            width: 1,
            syms: vec![0]
        };
        58
    ];
    syms[30] = xkb::KeySymMap {
        kt_index: [1, 0, 0, 0],
        group_info: 1,
        width: 2,
        syms: if mode == "mismatch" {
            vec![b'x' as u32, b'X' as u32]
        } else {
            vec![b'a' as u32, b'A' as u32]
        },
    };
    syms[42].syms = vec![if mode == "shift-symbol" {
        b'x' as u32
    } else {
        0xffe1
    }];
    if mode == "dead" {
        syms[31].syms = vec![0xfe51];
    }
    if mode == "compose" {
        syms[31].syms = vec![0xff20];
    }
    if mode == "iso-level" {
        syms[31].syms = vec![0xfe03];
        syms[32].syms = vec![0xfe11];
    }
    let mut counts = vec![0; 58];
    counts[42] = 1;
    let shift: xkb::Action = xkb::SASetMods {
        type_: xkb::SAType::SET_MODS,
        flags: xkb::SA::CLEAR_LOCKS | xkb::SA::USE_MOD_MAP_MODS,
        mask: xproto::ModMask::SHIFT,
        real_mods: xproto::ModMask::SHIFT,
        vmods_high: Default::default(),
        vmods_low: Default::default(),
    }
    .into();
    let mut actions = vec![shift];
    if mode == "action" {
        counts[30] = 1;
        actions.push(shift);
    }
    let mut two_level = xkb::KeyType {
        mods_mask: xproto::ModMask::SHIFT,
        mods_mods: xproto::ModMask::SHIFT,
        mods_vmods: Default::default(),
        num_levels: 2,
        has_preserve: false,
        map: vec![xkb::KTMapEntry {
            active: true,
            mods_mask: xproto::ModMask::SHIFT,
            level: 1,
            mods_mods: xproto::ModMask::SHIFT,
            mods_vmods: Default::default(),
        }],
        preserve: Vec::new(),
    };
    if mode == "type" {
        two_level.mods_vmods = xkb::VMod::from(1u16);
    }
    xkb::GetMapReply {
        device_id: device,
        sequence: 0,
        length: 0,
        min_key_code: 8,
        max_key_code: 65,
        first_type: 0,
        n_types: 2,
        total_types: 2,
        first_key_sym: 8,
        total_syms: 59,
        n_key_syms: 58,
        first_key_action: 8,
        total_actions: actions.len() as u16,
        n_key_actions: 58,
        first_key_behavior: 8,
        n_key_behaviors: 58,
        total_key_behaviors: 0,
        first_key_explicit: 8,
        n_key_explicit: 58,
        total_key_explicit: 0,
        first_mod_map_key: 8,
        n_mod_map_keys: 58,
        total_mod_map_keys: 1,
        first_v_mod_map_key: 8,
        n_v_mod_map_keys: 58,
        total_v_mod_map_keys: 0,
        virtual_mods: Default::default(),
        map: xkb::GetMapMap {
            types_rtrn: Some(vec![
                xkb::KeyType {
                    mods_mask: Default::default(),
                    mods_mods: Default::default(),
                    mods_vmods: Default::default(),
                    num_levels: 1,
                    has_preserve: false,
                    map: Vec::new(),
                    preserve: Vec::new(),
                },
                two_level,
            ]),
            syms_rtrn: Some(syms),
            key_actions: Some(xkb::GetMapMapKeyActions {
                acts_rtrn_count: counts,
                acts_rtrn_acts: actions,
            }),
            behaviors_rtrn: Some(Vec::new()),
            vmods_rtrn: Some(Vec::new()),
            explicit_rtrn: Some(Vec::new()),
            modmap_rtrn: Some(vec![xkb::KeyModMap {
                keycode: 50,
                mods: xproto::ModMask::SHIFT,
            }]),
            vmodmap_rtrn: Some(Vec::new()),
        },
    }
}
