#!/usr/bin/env python3
"""Exercise literal typing, paste, and cleanup through public Wayland MCP paths.

This harness constructs a private fake D-Bus portal, Klipper clipboard, and
keyboard/layout model. It never opens a GUI or connects to a desktop session.
"""

from __future__ import annotations

import argparse
import ctypes
import errno
import json
import os
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

INITIAL_FIELD = "LEFT_OLD_RIGHT"
INITIAL_CLIPBOARD = "PREVIOUS_CLIPBOARD"
TEST_TEXT = "LITERAL_λ!"
USER_CLIPBOARD = "NEW_USER_CLIPBOARD"
PORTAL_SERVICE = "org.freedesktop.portal.Desktop"
PORTAL_PATH = "/org/freedesktop/portal/desktop"
REMOTE_IFACE = "org.freedesktop.portal.RemoteDesktop"
REQUEST_IFACE = "org.freedesktop.portal.Request"
SESSION_IFACE = "org.freedesktop.portal.Session"
KLIPPER_SERVICE = "org.kde.klipper"
KLIPPER_PATH = "/klipper"
KLIPPER_IFACE = "org.kde.klipper.klipper"
CONTROL_SERVICE = "org.computeruse.PrivateFixture"
CONTROL_PATH = "/org/computeruse/PrivateFixture"
CONTROL_IFACE = "org.computeruse.PrivateFixture"
WINDOW_SERVICE = "dev.avifenesh.ComputerUseLinux.WindowControl"
WINDOW_PATH = "/dev/avifenesh/ComputerUseLinux/WindowControl"
TARGET_WINDOW = 1001
OTHER_WINDOW = 1002
OTHER_FIELD = "OTHER_ORIGINAL"

children: list[subprocess.Popen[bytes]] = []
log_handles: list[Any] = []


def become_subreaper() -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))


def child_env(root: Path, *, bus_address: str = "", backend_path: str = "") -> dict[str, str]:
    env = {
        "PATH": f"{root / 'bin'}:/usr/bin:/bin",
        "HOME": os.path.expanduser("~"),
        "LANG": "C.UTF-8",
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "data"),
        "XDG_STATE_HOME": str(root / "state"),
        "XDG_RUNTIME_DIR": str(root / "runtime"),
        "TMPDIR": str(root / "tmp"),
        "XDG_CURRENT_DESKTOP": "KDE",
        "XDG_SESSION_TYPE": "wayland",
        "DESKTOP_SESSION": "KDE",
        "XDG_SESSION_DESKTOP": "KDE",
        "DISPLAY": ":98765",
        "XAUTHORITY": str(root / "runtime" / "no-host-xauthority"),
        "HYPRLAND_INSTANCE_SIGNATURE": "private-fixture-no-hyprland",
        "WAYLAND_DISPLAY": "fixture-wayland-does-not-exist",
        "COMPUTER_USE_LINUX_FORCE_PORTAL_KEYBOARD": "1",
        "CU_DISABLE_ABS_POINTER": "1",
        "YDOTOOL_SOCKET": str(root / "runtime" / "no-ydotool.sock"),
        "PASTE_FIXTURE_SCENARIO": os.environ.get("PASTE_FIXTURE_SCENARIO", "paste"),
    }
    if env["PASTE_FIXTURE_SCENARIO"] == "targeted-no-screenshot":
        env["COMPUTER_USE_LINUX_SCREENSHOT_BACKEND"] = "portal"
    if bus_address:
        env["DBUS_SESSION_BUS_ADDRESS"] = bus_address
    if backend_path:
        env["PASTE_FIXTURE_STATE"] = backend_path
    if env["PASTE_FIXTURE_SCENARIO"] in ("gnome-portal-with-raw", "gnome-forced-raw", "gnome-cancel-start", "gnome-no-literal-backend", "gnome-cancel-raw-probe"):
        env.update({
            "XDG_CURRENT_DESKTOP": "GNOME",
            "DESKTOP_SESSION": "gnome",
            "XDG_SESSION_DESKTOP": "gnome",
            "YDOTOOL_SOCKET": str(root / "runtime" / "raw.sock"),
            "PASTE_FIXTURE_ROOT": str(root),
            "PASTE_FIXTURE_RAW_TRACE": str(root / "raw-cli.jsonl"),
        })
        env.pop("COMPUTER_USE_LINUX_FORCE_PORTAL_KEYBOARD")
        if env["PASTE_FIXTURE_SCENARIO"] in ("gnome-forced-raw", "gnome-cancel-raw-probe"):
            env["COMPUTER_USE_LINUX_FORCE_YDOTOOL_KEYBOARD"] = "1"
    if env["PASTE_FIXTURE_SCENARIO"] == "wtype-after-portal-denial":
        env.update({
            "XDG_CURRENT_DESKTOP": "sway",
            "DESKTOP_SESSION": "sway",
            "XDG_SESSION_DESKTOP": "sway",
            "PASTE_FIXTURE_ROOT": str(root),
        })
        env.pop("COMPUTER_USE_LINUX_FORCE_PORTAL_KEYBOARD")
    return env


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def private_bus_config(root: Path) -> Path:
    path = root / "config" / "dbus" / "session.conf"
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_text(
        f"""<busconfig>
  <type>session</type>
  <listen>unix:path={root / 'runtime' / 'bus'}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
""",
        encoding="utf-8",
    )
    return path


def install_ydotool_deny_shim(root: Path) -> None:
    path = root / "bin" / "ydotool"
    path.write_text(
        "#!/bin/sh\nprintf '%s\\n' 'ydotool forbidden by synthetic fixture' >&2\nexit 125\n",
        encoding="utf-8",
    )
    path.chmod(0o700)


def install_supported_raw_shim(root: Path) -> None:
    path = root / "bin" / "ydotool"
    path.write_text('''#!/usr/bin/python3
import json
import os
from pathlib import Path
import socket
import sys
import time
args = sys.argv[1:]
if args and args[0] in ('help', '--help'):
    print('Usage: ydotool <cmd> <args>\\nAvailable commands:\\n  click\\n  mousemove\\n  type\\n  key\\n  debug')
    sys.exit(0)
root = Path(os.environ['PASTE_FIXTURE_ROOT']).resolve(strict=True)
endpoint = Path(os.environ['YDOTOOL_SOCKET']).resolve(strict=True)
if not endpoint.is_relative_to(root) or not endpoint.is_socket():
    sys.exit(125)
probe = 'XDG_RUNTIME_DIR' not in os.environ
event = {'event': 'raw_cli', 'argv': args, 'probe': probe, 'socket': str(endpoint)}
delayed_probe = probe and args == ['mousemove', '--absolute', '--', '0', '0'] and os.environ.get('PASTE_FIXTURE_SCENARIO') == 'gnome-cancel-raw-probe'
if delayed_probe:
    event['delayed_reply'] = True
if probe:
    if not args or args[0] not in ('mousemove', 'click', 'key', 'type'):
        sys.exit(125)
elif args == ['type', '--file', '-'] and endpoint == root / 'runtime' / 'raw.sock':
    event['text'] = sys.stdin.read()
    # This is a fixture record, never a native input_event payload. The
    # receiver belongs to this test and has no connection to an input device.
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sender:
        sender.connect(str(endpoint))
        sender.send(json.dumps(event, ensure_ascii=False).encode('utf-8'))
else:
    sys.exit(125)
with open(os.environ['PASTE_FIXTURE_RAW_TRACE'], 'a', encoding='utf-8') as trace:
    trace.write(json.dumps(event, ensure_ascii=False) + '\\n')
if delayed_probe:
    time.sleep(.35)
''', encoding="utf-8")
    path.chmod(0o700)


def install_recording_wtype_shim(root: Path) -> None:
    path = root / "bin" / "wtype"
    path.write_text('''#!/usr/bin/python3
import dbus
import os
from pathlib import Path
import sys
root = Path(os.environ['PASTE_FIXTURE_ROOT']).resolve(strict=True)
if sys.argv[1:] != ['-'] or os.environ['DBUS_SESSION_BUS_ADDRESS'] != f'unix:path={root / "runtime" / "bus"}':
    sys.exit(125)
text = sys.stdin.read()
bus = dbus.SessionBus()
control = dbus.Interface(bus.get_object('org.computeruse.PrivateFixture', '/org/computeruse/PrivateFixture'), 'org.computeruse.PrivateFixture')
control.RecordWtype(text)
''', encoding="utf-8")
    path.chmod(0o700)


def start_child(root: Path, name: str, argv: list[str], env: dict[str, str], **kwargs: Any) -> subprocess.Popen[bytes]:
    logs = root / "logs"
    logs.mkdir(mode=0o700, parents=True, exist_ok=True)
    handle = (logs / f"{name}.log").open("ab", buffering=0)
    log_handles.append(handle)
    proc = subprocess.Popen(
        argv,
        env=env,
        stdin=kwargs.pop("stdin", subprocess.DEVNULL),
        stdout=kwargs.pop("stdout", handle),
        stderr=kwargs.pop("stderr", subprocess.STDOUT),
        close_fds=True,
        start_new_session=True,
        **kwargs,
    )
    children.append(proc)
    return proc


def wait_until(description: str, predicate: Any, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        for proc in children:
            if proc.poll() is not None:
                raise RuntimeError(f"{description}: child PID {proc.pid} exited with {proc.returncode}")
        time.sleep(0.025)
    raise TimeoutError(f"timed out waiting for {description}")


def dbus_name_path(sender: str) -> str:
    return sender.lstrip(":").replace(".", "_")


def make_request_path(sender: str, token: str) -> str:
    return f"/org/freedesktop/portal/desktop/request/{dbus_name_path(sender)}/{token}"


def make_session_path(sender: str, token: str) -> str:
    return f"/org/freedesktop/portal/desktop/session/{dbus_name_path(sender)}/{token}"


class FixtureState:
    def __init__(self, state_path: Path) -> None:
        self.state_path = state_path
        self.field = INITIAL_FIELD
        self.selection = [5, 8]
        self.clipboard = INITIAL_CLIPBOARD
        self.held: set[str] = set()
        self.trace: list[dict[str, Any]] = []
        self.sessions: set[str] = set()
        self.focused_window = (
            OTHER_WINDOW
            if os.environ.get("PASTE_FIXTURE_SCENARIO") in ("wtype-after-portal-denial", "targeted-no-screenshot")
            else TARGET_WINDOW
        )
        self.other_field = OTHER_FIELD
        self.other_selection = [0, len(OTHER_FIELD)]
        self.save()

    def save(self) -> None:
        atomic_json(
            self.state_path,
            {
                "fixture_kind": "constructed_fake_portal_klipper_layout",
                "field": self.field,
                "selection": self.selection,
                "clipboard": self.clipboard,
                "held_keys": sorted(self.held),
                "trace": self.trace,
                "sessions": sorted(self.sessions),
                "focused_window": self.focused_window,
                "other_field": self.other_field,
            },
        )

    def clipboard_get(self) -> str:
        self.trace.append({"event": "clipboard_get", "value": self.clipboard})
        self.save()
        return self.clipboard

    def clipboard_set(self, value: str) -> None:
        self.clipboard = value
        self.trace.append({"event": "clipboard_set", "value": value, "held_keys": sorted(self.held)})
        self.save()

    @staticmethod
    def _key_identity(route: str, code: int) -> str:
        return f"{route}:{code}"

    def _is_control_held(self) -> bool:
        return "keycode:29" in self.held or "keysym:65507" in self.held

    def _is_shift_held(self) -> bool:
        return "keycode:42" in self.held or "keysym:65505" in self.held

    def _replace_selection_from_clipboard(self) -> None:
        if self.focused_window == TARGET_WINDOW:
            start, end = self.selection
            self.field = self.field[:start] + self.clipboard + self.field[end:]
            caret = start + len(self.clipboard)
            self.selection = [caret, caret]
        elif self.focused_window == OTHER_WINDOW:
            start, end = self.other_selection
            self.other_field = self.other_field[:start] + self.clipboard + self.other_field[end:]
            caret = start + len(self.clipboard)
            self.other_selection = [caret, caret]
        else:
            raise AssertionError("paste has no constructed focused field")
        self.trace.append({"event": "semantic_paste", "text": self.clipboard, "window_id": self.focused_window})
        if os.environ.get("PASTE_FIXTURE_SCENARIO") == "clipboard-changed":
            self.clipboard = USER_CLIPBOARD
            self.trace.append({"event": "external_clipboard_change", "value": USER_CLIPBOARD})

    def keyboard_event(self, route: str, code: int, state: int) -> None:
        identity = self._key_identity(route, code)
        pressed = int(state) == 1
        if pressed:
            self.held.add(identity)
        else:
            self.held.discard(identity)
        event = {"event": "key", "route": route, "code": int(code), "state": int(state), "held": sorted(self.held), "window_id": self.focused_window}
        if route == "keycode":
            event["layout_symbol"] = {29: "Control_L", 42: "Shift_L", 47: "k"}.get(int(code), "other")
        self.trace.append(event)

        if pressed and route == "keysym":
            control = self._is_control_held()
            shift = self._is_shift_held()
            # The fixture models logical XKB symbols separately from physical
            # evdev positions: Control+v, Control+Shift+v, and Shift+Insert
            # mean paste; physical evdev 47 maps to Programmer Dvorak's k.
            if (code == 118 and control) or (code == 65379 and shift):
                self._replace_selection_from_clipboard()
            elif not control and not shift and os.environ.get("PASTE_FIXTURE_SCENARIO") in ("gnome-portal-with-raw", "gnome-forced-raw", "gnome-cancel-start", "gnome-no-literal-backend", "gnome-cancel-raw-probe"):
                # The tested repertoire uses Latin-1, Greek lambda's legacy
                # X11 keysym, or the direct Unicode keysym encoding.
                character = None
                if 0x20 <= code <= 0x7E or 0xA0 <= code <= 0xFF:
                    character = chr(code)
                elif code == 0x07EB:
                    character = "λ"
                elif code & 0xFF000000 == 0x01000000:
                    character = chr(code & 0x00FFFFFF)
                if character is not None:
                    start, end = self.selection
                    self.field = self.field[:start] + character + self.field[end:]
                    caret = start + len(character)
                    self.selection = [caret, caret]
                    self.trace.append({"event": "literal_keysym", "text": character})
        self.save()


class ResponseObject:
    def __init__(self, bus_name: Any, path: str, retained: dict[str, Any]) -> None:
        import dbus.service

        self.obj = _RequestObject(bus_name, path)
        retained[path] = self.obj


import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


class _RequestObject(dbus.service.Object):
    def __init__(self, bus_name: Any, path: str) -> None:
        super().__init__(bus_name, path)

    @dbus.service.signal(REQUEST_IFACE, signature="ua{sv}")
    def Response(self, response_code: int, results: dict[str, Any]) -> None:
        pass


class _SessionObject(dbus.service.Object):
    def __init__(self, bus_name: Any, path: str, state: FixtureState) -> None:
        super().__init__(bus_name, path)
        self.state = state
        self.path = path

    @dbus.service.method(SESSION_IFACE, in_signature="", out_signature="", async_callbacks=("reply", "error"))
    def Close(self, reply: Any, error: Any) -> None:
        def close() -> bool:
            self.state.sessions.discard(self.path)
            self.state.held.clear()
            self.state.trace.append({"event": "session_close", "path": self.path})
            self.state.save()
            reply()
            return GLib.SOURCE_REMOVE
        if os.environ.get("PASTE_FIXTURE_SCENARIO") == "release-delayed":
            GLib.timeout_add(800, close)
        else:
            close()


class _PortalObject(dbus.service.Object):
    def __init__(self, bus_name: Any, state: FixtureState, retained: dict[str, Any]) -> None:
        super().__init__(bus_name, PORTAL_PATH)
        self.bus_name = bus_name
        self.state = state
        self.retained = retained
        self.sessions: dict[str, _SessionObject] = {}

    def _response(self, sender: str, options: dict[str, Any]) -> str:
        token = str(options["handle_token"])
        path = make_request_path(sender, token)
        obj = ResponseObject(self.bus_name, path, self.retained).obj
        return path

    def _schedule_response(self, path: str, results: dict[str, Any], delay_ms: int = 5, trace_event: str | None = None, response_code: int = 0) -> None:
        def emit() -> bool:
            obj = self.retained.get(path)
            if obj is not None:
                if trace_event:
                    self.state.trace.append({"event": trace_event})
                    self.state.save()
                obj.Response(dbus.UInt32(response_code), results)
            GLib.timeout_add(1000, lambda: self.retained.pop(path, None) is not None and False)
            return GLib.SOURCE_REMOVE

        GLib.timeout_add(delay_ms, emit)

    def _validate_session(self, session: str) -> None:
        if str(session) not in self.sessions:
            raise dbus.exceptions.DBusException(f"unknown fixture session {session}")

    @dbus.service.method("org.freedesktop.portal.Screenshot", in_signature="sa{sv}", out_signature="o", sender_keyword="sender")
    def Screenshot(self, parent_window: str, options: dict[str, Any], sender: str) -> dbus.ObjectPath:
        request = self._response(sender, options)
        self.state.trace.append({"event": "Screenshot", "parent_window": str(parent_window)})
        self.state.save()
        self._schedule_response(request, dbus.Dictionary({}, signature="sv"), response_code=1)
        return dbus.ObjectPath(request)

    @dbus.service.method(REMOTE_IFACE, in_signature="a{sv}", out_signature="o", sender_keyword="sender")
    def CreateSession(self, options: dict[str, Any], sender: str) -> dbus.ObjectPath:
        request = self._response(sender, options)
        session_token = str(options["session_handle_token"])
        session_path = make_session_path(sender, session_token)
        self.sessions[session_path] = _SessionObject(self.bus_name, session_path, self.state)
        self.state.sessions.add(session_path)
        self.state.trace.append({"event": "CreateSession", "sender": sender, "request": request, "session": session_path})
        self.state.save()
        self._schedule_response(
            request,
            dbus.Dictionary(
                {"session_handle": dbus.String(session_path, variant_level=1)},
                signature="sv",
            ),
        )
        return dbus.ObjectPath(request)

    @dbus.service.method(REMOTE_IFACE, in_signature="oa{sv}", out_signature="o", sender_keyword="sender")
    def SelectDevices(self, session: str, options: dict[str, Any], sender: str) -> dbus.ObjectPath:
        self._validate_session(session)
        if int(options.get("types", 0)) != 1:
            raise dbus.exceptions.DBusException("fixture expected keyboard device type 1")
        request = self._response(sender, options)
        self.state.trace.append({"event": "SelectDevices", "sender": sender, "session": str(session), "types": 1, "request": request})
        self.state.save()
        self._schedule_response(request, dbus.Dictionary({}, signature="sv"))
        return dbus.ObjectPath(request)

    @dbus.service.method(REMOTE_IFACE, in_signature="osa{sv}", out_signature="o", sender_keyword="sender")
    def Start(self, session: str, parent_window: str, options: dict[str, Any], sender: str) -> dbus.ObjectPath:
        self._validate_session(session)
        request = self._response(sender, options)
        self.state.trace.append({"event": "Start", "sender": sender, "session": str(session), "parent_window": str(parent_window), "request": request})
        self.state.save()
        if os.environ.get("PASTE_FIXTURE_SCENARIO") == "wtype-after-portal-denial":
            self.state.focused_window = OTHER_WINDOW
            self.state.trace.append({"event": "portal_focus_change", "focused_window": OTHER_WINDOW})
            self.state.save()
            self._schedule_response(request, dbus.Dictionary({}, signature="sv"), delay_ms=50, trace_event="StartDenied", response_code=2)
            return dbus.ObjectPath(request)
        self._schedule_response(
            request,
            dbus.Dictionary({"devices": dbus.UInt32(1, variant_level=1)}, signature="sv"),
            delay_ms=500 if os.environ.get("PASTE_FIXTURE_SCENARIO") == "gnome-cancel-start" else 5,
            trace_event="StartResponse" if os.environ.get("PASTE_FIXTURE_SCENARIO") == "gnome-cancel-start" else None,
        )
        return dbus.ObjectPath(request)

    @dbus.service.method(REMOTE_IFACE, in_signature="oa{sv}iu", out_signature="", async_callbacks=("reply", "error"))
    def NotifyKeyboardKeysym(self, session: str, options: dict[str, Any], keysym: int, state: int, reply: Any, error: Any) -> None:
        self._validate_session(session)
        scenario = os.environ.get("PASTE_FIXTURE_SCENARIO")
        if scenario == "release-delayed" and int(keysym) == 118 and int(state) == 0:
            def release() -> bool:
                self.state.keyboard_event("keysym", int(keysym), int(state))
                reply()
                return GLib.SOURCE_REMOVE
            GLib.timeout_add(1700, release)
            return
        self.state.keyboard_event("keysym", int(keysym), int(state))
        if scenario in ("portal-error", "release-delayed") and int(keysym) == 118 and int(state) == 1:
            error(dbus.exceptions.DBusException("constructed failure after paste submission"))
        elif scenario == "cancel-dispatched" and int(keysym) == 65507 and int(state) == 1:
            GLib.timeout_add(350, lambda: (reply(), GLib.SOURCE_REMOVE)[1])
        else:
            reply()

    @dbus.service.method(REMOTE_IFACE, in_signature="oa{sv}iu", out_signature="")
    def NotifyKeyboardKeycode(self, session: str, options: dict[str, Any], keycode: int, state: int) -> None:
        self._validate_session(session)
        self.state.keyboard_event("keycode", int(keycode), int(state))


class _KlipperObject(dbus.service.Object):
    def __init__(self, bus_name: Any, state: FixtureState) -> None:
        super().__init__(bus_name, KLIPPER_PATH)
        self.state = state

    @dbus.service.method(KLIPPER_IFACE, in_signature="", out_signature="s")
    def getClipboardContents(self) -> str:
        return self.state.clipboard_get()

    @dbus.service.method(KLIPPER_IFACE, in_signature="s", out_signature="", async_callbacks=("reply", "error"))
    def setClipboardContents(self, text: str, reply: Any, error: Any) -> None:
        self.state.clipboard_set(str(text))
        if os.environ.get("PASTE_FIXTURE_SCENARIO") == "cancel-prepared" and str(text) == TEST_TEXT:
            # Klipper has accepted the text, but its reply is still pending.
            # The caller can cancel here without racing a completed shortcut.
            GLib.timeout_add(350, lambda: (reply(), GLib.SOURCE_REMOVE)[1])
        elif os.environ.get("PASTE_FIXTURE_SCENARIO") == "write-error" and str(text) == TEST_TEXT:
            error(dbus.exceptions.DBusException("constructed error after clipboard mutation", name="org.computeruse.PrivateFixture.WriteFailed"))
        else:
            reply()


class _FixtureControlObject(dbus.service.Object):
    def __init__(self, bus_name: Any, state: FixtureState) -> None:
        super().__init__(bus_name, CONTROL_PATH)
        self.state = state

    @dbus.service.method(CONTROL_IFACE, in_signature="", out_signature="s")
    def GetField(self) -> str:
        return self.state.field

    @dbus.service.method(CONTROL_IFACE, in_signature="", out_signature="s")
    def GetClipboard(self) -> str:
        return self.state.clipboard

    @dbus.service.method(CONTROL_IFACE, in_signature="", out_signature="as")
    def GetHeldKeys(self) -> list[str]:
        return sorted(self.state.held)

    @dbus.service.method(CONTROL_IFACE, in_signature="", out_signature="as")
    def GetTrace(self) -> list[str]:
        return [json.dumps(event, ensure_ascii=False, sort_keys=True) for event in self.state.trace]


    @dbus.service.method(CONTROL_IFACE, in_signature="", out_signature="s")
    def GetWindowState(self) -> str:
        return json.dumps({"focused_window": self.state.focused_window, "other_field": self.state.other_field})

    @dbus.service.method(CONTROL_IFACE, in_signature="s", out_signature="")
    def RecordWtype(self, text: str) -> None:
        destination = self.state.focused_window
        self.state.trace.append({"event": "wtype", "text": str(text), "destination_window": destination})
        if destination == TARGET_WINDOW:
            start, end = self.state.selection
            self.state.field = self.state.field[:start] + str(text) + self.state.field[end:]
        elif destination == OTHER_WINDOW:
            self.state.other_field += str(text)
        else:
            raise dbus.exceptions.DBusException("recording wtype has no constructed destination")
        self.state.save()


class _WindowControlObject(dbus.service.Object):
    def __init__(self, bus_name: Any, state: FixtureState) -> None:
        super().__init__(bus_name, WINDOW_PATH)
        self.state = state

    @dbus.service.method(WINDOW_SERVICE, in_signature="", out_signature="s")
    def ListWindows(self) -> str:
        self.state.trace.append({"event": "ListWindows", "focused_window": self.state.focused_window})
        self.state.save()
        return json.dumps([
            {"window_id": window, "title": title, "app_id": app_id, "wm_class": None,
             "pid": os.getpid(), "bounds": {"x": 0, "y": 0, "width": 200, "height": 100},
             "workspace": 0, "focused": self.state.focused_window == window,
             "hidden": False, "client_type": "wayland", "backend": "fixture"}
            for window, title, app_id in ((TARGET_WINDOW, "Target field", "fixture-target"), (OTHER_WINDOW, "Other field", "fixture-other"))
        ])

    @dbus.service.method(WINDOW_SERVICE, in_signature="t", out_signature="bs")
    def ActivateWindow(self, window: int) -> tuple[bool, str]:
        if int(window) not in (TARGET_WINDOW, OTHER_WINDOW):
            return False, "unknown constructed window"
        self.state.focused_window = int(window)
        self.state.trace.append({"event": "ActivateWindow", "window_id": int(window)})
        self.state.save()
        return True, "activated constructed window"


def backend_main(state_path: Path, ready_path: Path) -> int:
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    names = {
        name: dbus.service.BusName(name, bus=bus, do_not_queue=True)
        for name in (KLIPPER_SERVICE, CONTROL_SERVICE)
    }
    state = FixtureState(state_path)
    retained: dict[str, Any] = {}
    # The bus config has no activation directories, so an omitted portal
    # service is unavailable throughout this scenario.
    if os.environ.get("PASTE_FIXTURE_SCENARIO") != "gnome-no-literal-backend":
        names[PORTAL_SERVICE] = dbus.service.BusName(PORTAL_SERVICE, bus=bus, do_not_queue=True)
        _PortalObject(names[PORTAL_SERVICE], state, retained)
    _KlipperObject(names[KLIPPER_SERVICE], state)
    _FixtureControlObject(names[CONTROL_SERVICE], state)
    if os.environ.get("PASTE_FIXTURE_SCENARIO") in ("wtype-after-portal-denial", "targeted-no-screenshot"):
        names[WINDOW_SERVICE] = dbus.service.BusName(WINDOW_SERVICE, bus=bus, do_not_queue=True)
        _WindowControlObject(names[WINDOW_SERVICE], state)
    loop = GLib.MainLoop()
    ready_path.write_text("ready\n", encoding="utf-8")
    try:
        loop.run()
    except KeyboardInterrupt:
        pass
    return 0


def inspect_backend(env: dict[str, str]) -> dict[str, Any]:
    import dbus

    bus = dbus.SessionBus()
    obj = bus.get_object(CONTROL_SERVICE, CONTROL_PATH)
    iface = dbus.Interface(obj, CONTROL_IFACE)
    trace = [json.loads(str(line)) for line in iface.GetTrace()]
    snapshot = {
        "portal_owner_present": bool(bus.name_has_owner(PORTAL_SERVICE)),
        "field": str(iface.GetField()),
        "clipboard": str(iface.GetClipboard()),
        "held_keys": [str(key) for key in iface.GetHeldKeys()],
        "trace": trace,
    }
    if os.environ.get("PASTE_FIXTURE_SCENARIO") in ("wtype-after-portal-denial", "targeted-no-screenshot"):
        snapshot.update(json.loads(str(iface.GetWindowState())))
    return snapshot


def inspect_fixture_from_runner(root: Path, bus_address: str) -> dict[str, Any]:
    inspector = start_child(
        root,
        "fixture-inspector",
        [sys.executable, str(Path(__file__).resolve()), "--inspect"],
        child_env(root, bus_address=bus_address),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout, stderr = inspector.communicate(timeout=5)
    if inspector.returncode != 0:
        raise RuntimeError(f"fixture control read failed: {stderr.decode('utf-8', 'replace')}")
    return json.loads(stdout.decode("utf-8"))


class JsonRpcClient:
    def __init__(self, proc: subprocess.Popen[bytes], timeout: float = 25.0) -> None:
        self.proc = proc
        self.timeout = timeout
        self.buffer = bytearray()

    def send(self, value: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(value, ensure_ascii=False).encode("utf-8") + b"\n")
        self.proc.stdin.flush()

    def receive(self, expected_id: int) -> dict[str, Any]:
        assert self.proc.stdout is not None
        deadline = time.monotonic() + self.timeout
        fd = self.proc.stdout.fileno()
        while time.monotonic() < deadline:
            newline = self.buffer.find(b"\n")
            if newline >= 0:
                raw = bytes(self.buffer[:newline]).strip()
                del self.buffer[: newline + 1]
                if not raw:
                    continue
                message = json.loads(raw.decode("utf-8"))
                if message.get("id") == expected_id:
                    return message
                continue
            if self.proc.poll() is not None:
                raise RuntimeError(f"MCP server exited with status {self.proc.returncode}")
            remaining = max(0.0, deadline - time.monotonic())
            readable, _, _ = select.select([fd], [], [], min(0.1, remaining))
            if readable:
                chunk = os.read(fd, 65536)
                if not chunk:
                    raise EOFError("MCP server closed stdout")
                self.buffer.extend(chunk)
        raise TimeoutError(f"timed out waiting for MCP JSON-RPC id {expected_id}")

    def rpc(self, request_id: int, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        response = self.receive(request_id)
        if "error" in response:
            raise RuntimeError(f"MCP {method} failed: {response['error']}")
        return response.get("result", {})


def call_mcp(binary: Path, env: dict[str, str], root: Path) -> dict[str, Any]:
    mcp = start_child(
        root, "mcp-server", [str(binary), "mcp"], env,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
    )
    client = JsonRpcClient(mcp)
    initialized = client.rpc(
        1,
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "synthetic-paste-route-test", "version": "1"},
        },
    )
    if not initialized.get("protocolVersion"):
        raise RuntimeError("MCP initialize response omitted protocolVersion")
    client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    if env["PASTE_FIXTURE_SCENARIO"] in ("cancel-prepared", "cancel-dispatched", "gnome-cancel-start", "gnome-cancel-raw-probe"):
        client.send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "type_text", "arguments": {"text": TEST_TEXT}}})
        state_path = root / "fixture-state.json"
        if env["PASTE_FIXTURE_SCENARIO"] == "cancel-prepared":
            wait_until("prepared clipboard", lambda: json.loads(state_path.read_text())["clipboard"] == TEST_TEXT)
        elif env["PASTE_FIXTURE_SCENARIO"] == "gnome-cancel-start":
            wait_until("portal Start request", lambda: any(event["event"] == "Start" for event in json.loads(state_path.read_text())["trace"]))
        elif env["PASTE_FIXTURE_SCENARIO"] == "gnome-cancel-raw-probe":
            trace_path = root / "raw-cli.jsonl"
            wait_until("delayed raw capability probe", lambda: trace_path.exists() and any(json.loads(line).get("delayed_reply") for line in trace_path.read_text().splitlines()))
            before_cancel = json.loads(state_path.read_text())
            raw_before = [json.loads(line) for line in trace_path.read_text().splitlines()]
            assert all(event["probe"] for event in raw_before), "raw typing began before the cancellation trigger"
            assert before_cancel["field"] == INITIAL_FIELD and not before_cancel["held_keys"], "input changed before cancellation"
            atomic_json(root / "cancel-observation.json", {
                "probe_before_notification": raw_before[-1],
                "runtime_raw_before_notification": [],
                "field_before_notification": before_cancel["field"],
                "held_keys_before_notification": before_cancel["held_keys"],
            })
        else:
            wait_until("keyboard dispatch", lambda: bool(json.loads(state_path.read_text())["held_keys"]))
        if env["PASTE_FIXTURE_SCENARIO"] == "gnome-cancel-start":
            before_cancel = json.loads(state_path.read_text())
            events = [event["event"] for event in before_cancel["trace"]]
            assert "StartResponse" not in events and "key" not in events, "portal input began before the cancellation trigger"
            assert before_cancel["field"] == INITIAL_FIELD, "field changed before cancellation"
            atomic_json(root / "cancel-observation.json", {
                "trace_before_notification": events,
                "field_before_notification": before_cancel["field"],
                "held_keys_before_notification": before_cancel["held_keys"],
            })
        client.send({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2, "reason": "constructed request cancellation"}})
        response = client.receive(2)
        if env["PASTE_FIXTURE_SCENARIO"] == "gnome-cancel-start":
            # A prompt cancellation response must not conceal a detached
            # input operation that resumes after the delayed portal reply.
            wait_until("delayed portal Start response", lambda: any(event["event"] == "StartResponse" for event in json.loads(state_path.read_text())["trace"]))
            time.sleep(.25)
        elif env["PASTE_FIXTURE_SCENARIO"] == "gnome-cancel-raw-probe":
            # Observe after support resolution as well as after the MCP reply,
            # so caller cancellation cannot conceal a detached typing command.
            trace_path = root / "raw-cli.jsonl"
            wait_until("final raw capability probe", lambda: any(event["probe"] and event["argv"] == ["type", "--file", "-"] for event in (json.loads(line) for line in trace_path.read_text().splitlines())))
            time.sleep(.25)
        if "error" in response:
            return {"rpc_error": response["error"]}
        return response.get("result", {})
    arguments: dict[str, Any] = {"text": TEST_TEXT}
    if env["PASTE_FIXTURE_SCENARIO"] in ("wtype-after-portal-denial", "targeted-no-screenshot"):
        arguments["window_id"] = TARGET_WINDOW
    return client.rpc(2, "tools/call", {"name": "type_text", "arguments": arguments})


def terminate_children() -> None:
    for proc in reversed(children):
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 3.0
    while time.monotonic() < deadline and any(proc.poll() is None for proc in children):
        time.sleep(0.025)
    for proc in reversed(children):
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    for proc in reversed(children):
        try:
            proc.wait(timeout=1)
        except (subprocess.TimeoutExpired, ChildProcessError):
            pass
    reap_deadline = time.monotonic() + 2.0
    while time.monotonic() < reap_deadline:
        try:
            pid, _status = os.waitpid(-1, os.WNOHANG)
        except (ChildProcessError, OSError) as exc:
            if isinstance(exc, ChildProcessError) or exc.errno == errno.ECHILD:
                break
            raise
        if pid == 0:
            time.sleep(0.025)
    for handle in log_handles:
        try:
            handle.close()
        except OSError:
            pass


def run(binary: Path, evidence_path: Path | None) -> int:
    become_subreaper()
    root = Path(tempfile.mkdtemp(prefix="paste-backend-test-", dir="/tmp"))
    os.chmod(root, 0o700)
    for name in ("home", "config", "cache", "data", "state", "runtime", "tmp", "bin", "logs"):
        path = root / name
        path.mkdir(mode=0o700)
        os.chmod(path, 0o700)
    os.chmod(root / "runtime", 0o700)
    install_ydotool_deny_shim(root)
    bus_conf = private_bus_config(root)
    state_path = root / "fixture-state.json"
    ready_path = root / "backend.ready"
    bus_address = f"unix:path={root / 'runtime' / 'bus'}"
    clean = child_env(root, bus_address=bus_address)
    report: dict[str, Any] = {
        "evidence_kind": "constructed_fake_portal_klipper_layout",
        "claim_scope": "synthetic backend behavior only; not a real KWin result",
        "expected": {
            "field": "LEFT_LITERAL_λ!_RIGHT",
            "clipboard": INITIAL_CLIPBOARD,
            "held_keys": [],
        },
    }
    scenario = clean["PASTE_FIXTURE_SCENARIO"]
    report["scenario"] = scenario
    if scenario == "targeted-no-screenshot":
        report["expected"].update({"focused_window": TARGET_WINDOW, "other_field": OTHER_FIELD})
    raw_receiver = None
    if scenario == "wtype-after-portal-denial":
        install_recording_wtype_shim(root)
        report["claim_scope"] = "constructed window-control DBus and wtype routing after portal denial; no compositor qualification"
        report["expected"].update({"focused_window": TARGET_WINDOW, "other_field": OTHER_FIELD})
    if scenario in ("gnome-portal-with-raw", "gnome-forced-raw", "gnome-cancel-start", "gnome-no-literal-backend", "gnome-cancel-raw-probe"):
        report["claim_scope"] = "synthetic GNOME-style portal routing and field behavior; no compositor qualification"
        install_supported_raw_shim(root)
        raw_receiver = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        raw_receiver.bind(clean["YDOTOOL_SOCKET"])
        raw_receiver.setblocking(False)
    if scenario in ("gnome-forced-raw", "gnome-cancel-start", "gnome-no-literal-backend", "gnome-cancel-raw-probe"):
        report["expected"]["field"] = INITIAL_FIELD
    if scenario in ("cancel-prepared", "write-error"):
        report["expected"]["field"] = INITIAL_FIELD
    if scenario == "clipboard-changed":
        report["expected"]["clipboard"] = USER_CLIPBOARD
    status = 1
    try:
        if scenario == "gnome-no-literal-backend":
            # Check the private child's PATH plus directories the server may
            # append. Refuse to start if a wtype file is visible; callers can
            # hide it in the disposable mount namespace before rerunning.
            wtype_paths = [root / "bin" / "wtype"] + [
                Path(directory) / "wtype"
                for directory in ("/run/current-system/sw/bin", "/usr/local/bin", "/usr/bin", "/bin")
            ]
            report["unavailable_literal_backends"] = {
                "portal": "service omitted on private bus with no activation directories",
                "wtype_files": {str(path): path.is_file() for path in wtype_paths},
                "raw_identity_channel": "none; fixture exposes only its recording datagram socket",
            }
            if any(path.is_file() for path in wtype_paths):
                raise RuntimeError("hide wtype in the disposable namespace before this scenario")
        bus = start_child(root, "dbus-daemon", ["/usr/bin/dbus-daemon", "--nofork", f"--config-file={bus_conf}"], clean)
        wait_until("private D-Bus socket", lambda: (root / "runtime" / "bus").is_socket())

        backend_env = child_env(root, bus_address=bus_address, backend_path=str(state_path))
        backend = start_child(
            root,
            "fake-backend",
            [sys.executable, str(Path(__file__).resolve()), "--backend", str(state_path), str(ready_path)],
            backend_env,
        )
        wait_until("fake backend D-Bus services", ready_path.exists)

        mcp_env = child_env(root, bus_address=bus_address)
        # Keep xdotool off the MCP child's initial PATH. The production
        # environment hydrator may append common command directories, but its
        # Wayland selection still rejects the XTEST route. The fake DISPLAY is
        # explicitly non-existent so it cannot inherit the host endpoint.
        mcp_env["PATH"] = str(root / "bin")
        report["mcp_environment"] = {
            "dbus_session_bus": "private fixture bus",
            "display": mcp_env["DISPLAY"],
            "wayland_display": mcp_env["WAYLAND_DISPLAY"],
            "at_spi_bus_address": "unset",
            "ydotool_socket": "task-private raw datagram socket" if raw_receiver else "private nonexistent socket",
            "desktop": mcp_env["XDG_CURRENT_DESKTOP"],
            "forced_raw": mcp_env.get("COMPUTER_USE_LINUX_FORCE_YDOTOOL_KEYBOARD") == "1",
        }
        result = call_mcp(binary, mcp_env, root)
        cancellation_observation = root / "cancel-observation.json"
        if cancellation_observation.exists():
            report["cancellation_trigger"] = json.loads(cancellation_observation.read_text())
        if result.get("isError"):
            report["mcp_result"] = result
            raise AssertionError("type_text MCP result marked isError")
        report["mcp_result"] = result

        observed = inspect_fixture_from_runner(root, bus_address)
        if raw_receiver is not None:
            datagrams = []
            while True:
                try:
                    datagrams.append(json.loads(raw_receiver.recv(65536)))
                except BlockingIOError:
                    break
            trace_path = root / "raw-cli.jsonl"
            observed["raw_cli"] = [json.loads(line) for line in trace_path.read_text().splitlines()] if trace_path.exists() else []
            observed["raw_datagrams"] = datagrams
        report["observed"] = observed
        status = 0
        failures = []
        for name, expected in report["expected"].items():
            actual = observed.get(name)
            if actual != expected:
                failures.append({"field": name, "expected": expected, "observed": actual})
        if scenario == "targeted-no-screenshot":
            inputs = [event for event in observed["trace"] if event["event"] in ("key", "semantic_paste")]
            if not inputs or any(event["window_id"] != TARGET_WINDOW for event in inputs):
                failures.append({"field": "input recipient", "expected": TARGET_WINDOW, "observed": inputs})
            requests = [event for event in observed["trace"] if event["event"] == "Screenshot"]
            if requests:
                failures.append({"field": "incidental screenshot requests", "expected": [], "observed": requests})
            if result.get("structuredContent", {}).get("ok") is not True:
                failures.append({"field": "MCP targeted typing ok", "expected": True, "observed": result})
        if scenario in ("cancel-prepared", "write-error", "portal-error", "release-delayed"):
            if scenario in ("cancel-prepared", "write-error") and any(event["event"] == "key" for event in observed["trace"]):
                failures.append({"field": "keyboard dispatch", "expected": "none after cancellation", "observed": "key events"})
            if result.get("structuredContent", {}).get("ok"):
                failures.append({"field": "MCP ok", "expected": False, "observed": True})
        if scenario in ("portal-error", "release-delayed", "cancel-dispatched"):
            paste_events = [event for event in observed["trace"] if event["event"] == "semantic_paste"]
            if len(paste_events) != 1:
                failures.append({"field": "paste submissions", "expected": 1, "observed": len(paste_events)})
        if scenario == "release-delayed":
            restores = [event for event in observed["trace"] if event["event"] == "clipboard_set" and event["value"] == INITIAL_CLIPBOARD]
            if not restores or any(event["held_keys"] for event in restores):
                failures.append({"field": "restore ordering", "expected": "restore after keyboard cleanup", "observed": restores})
        if scenario == "wtype-after-portal-denial":
            trace = observed["trace"]
            events = [event["event"] for event in trace]
            wtype = [event for event in trace if event["event"] == "wtype"]
            if result.get("structuredContent", {}).get("ok") is not True:
                failures.append({"field": "MCP literal fallback ok", "expected": True, "observed": result})
            if len(wtype) != 1 or wtype[0]["destination_window"] != TARGET_WINDOW or wtype[0]["text"] != TEST_TEXT:
                failures.append({"field": "wtype destination", "expected": {"count": 1, "window_id": TARGET_WINDOW, "text": TEST_TEXT}, "observed": wtype})
            try:
                start = events.index("Start")
                changed = events.index("portal_focus_change", start)
                denied = events.index("StartDenied", changed)
                assert any(event["event"] == "ActivateWindow" and event["window_id"] == TARGET_WINDOW for event in trace[:start])
                assert any(event["event"] == "ListWindows" and event["focused_window"] == TARGET_WINDOW for event in trace[:start])
                assert any(event["event"] == "ActivateWindow" and event["window_id"] == TARGET_WINDOW for event in trace[denied + 1:events.index("wtype")])
                assert any(event["event"] == "ListWindows" and event["focused_window"] == TARGET_WINDOW for event in trace[denied + 1:events.index("wtype")])
            except (ValueError, AssertionError):
                failures.append({"field": "target verification after denied portal", "expected": "target activated and freshly queried before and after portal focus change", "observed": trace})
            if any(event["event"] in ("key", "literal_keysym", "semantic_paste", "clipboard_set") for event in trace):
                failures.append({"field": "denied portal dispatch", "expected": "only one wtype submission", "observed": trace})
        if scenario in ("gnome-portal-with-raw", "gnome-forced-raw", "gnome-cancel-start", "gnome-no-literal-backend", "gnome-cancel-raw-probe"):
            if scenario in ("gnome-portal-with-raw", "gnome-forced-raw") and not result.get("structuredContent", {}).get("ok"):
                failures.append({"field": "MCP ok", "expected": True, "observed": result})
            raw_dispatch = [event for event in observed["raw_cli"] if not event["probe"]]
            if scenario == "gnome-portal-with-raw":
                literal = "".join(event["text"] for event in observed["trace"] if event["event"] == "literal_keysym")
                if literal != TEST_TEXT or raw_dispatch or observed["raw_datagrams"]:
                    failures.append({"field": "literal portal routing", "expected": "literal keysyms with no raw dispatch",
                                     "observed": {"literal": literal, "raw_dispatch": raw_dispatch, "datagrams": observed["raw_datagrams"]}})
                if any(event["event"].startswith("clipboard_") or event["event"] == "semantic_paste" for event in observed["trace"]):
                    failures.append({"field": "literal portal path", "expected": "no clipboard operations", "observed": observed["trace"]})
            elif scenario == "gnome-forced-raw":
                if len(raw_dispatch) != 1 or len(observed["raw_datagrams"]) != 1 or raw_dispatch[0].get("text") != TEST_TEXT:
                    failures.append({"field": "forced raw compatibility", "expected": "one recorded raw type dispatch",
                                     "observed": {"raw_dispatch": raw_dispatch, "datagrams": observed["raw_datagrams"]}})
                if any(event["event"] in ("CreateSession", "key", "literal_keysym") for event in observed["trace"]):
                    failures.append({"field": "forced raw compatibility", "expected": "no portal input", "observed": observed["trace"]})
            elif scenario == "gnome-cancel-start":
                if result.get("structuredContent", {}).get("ok"):
                    failures.append({"field": "MCP ok after cancellation", "expected": False, "observed": True})
                if raw_dispatch or observed["raw_datagrams"] or any(event["event"] in ("key", "literal_keysym") for event in observed["trace"]):
                    failures.append({"field": "cancelled Start dispatch", "expected": "no keyboard or raw input",
                                     "observed": {"portal_trace": observed["trace"], "raw_dispatch": raw_dispatch, "datagrams": observed["raw_datagrams"]}})
            elif scenario == "gnome-cancel-raw-probe":
                if result.get("structuredContent", {}).get("ok") is not False:
                    failures.append({"field": "MCP ok after raw-probe cancellation", "expected": False, "observed": result})
                if raw_dispatch or observed["raw_datagrams"] or any(event["event"] in ("key", "literal_keysym", "semantic_paste", "clipboard_set") for event in observed["trace"]):
                    failures.append({"field": "cancelled raw probe dispatch", "expected": "no input or clipboard mutation",
                                     "observed": {"portal_trace": observed["trace"], "raw_dispatch": raw_dispatch, "datagrams": observed["raw_datagrams"]}})
            elif scenario == "gnome-no-literal-backend":
                if observed["portal_owner_present"]:
                    failures.append({"field": "portal availability", "expected": False, "observed": True})
                if result.get("structuredContent", {}).get("ok") is not False:
                    failures.append({"field": "MCP ok without literal backend", "expected": False, "observed": result})
                if raw_dispatch or observed["raw_datagrams"] or any(event["event"] in ("key", "literal_keysym", "semantic_paste", "clipboard_set") for event in observed["trace"]):
                    failures.append({"field": "unverified automatic fallback", "expected": "no input or clipboard mutation",
                                     "observed": {"portal_trace": observed["trace"], "raw_dispatch": raw_dispatch, "datagrams": observed["raw_datagrams"]}})
        report["assertions"] = {"pass": not failures, "failures": failures}
        if failures:
            status = 1
    except Exception as exc:
        report["error"] = f"{type(exc).__name__}: {exc}"
        status = 1
    finally:
        if "observed" not in report and ready_path.exists():
            try:
                report["observed"] = inspect_fixture_from_runner(root, bus_address)
            except Exception as exc:
                report["fixture_read_error"] = f"{type(exc).__name__}: {exc}"
        terminate_children()
        if raw_receiver is not None:
            raw_receiver.close()
        report["exit_status"] = status
        if evidence_path is not None:
            evidence_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            evidence_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False, indent=2), flush=True)
        shutil.rmtree(root, ignore_errors=True)
    return status


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, help="path to computer-use-linux MCP binary")
    parser.add_argument("--save-evidence", type=Path, help="optional path for synthetic-only JSON result")
    parser.add_argument("--scenario", choices=("paste", "cancel-prepared", "cancel-dispatched", "clipboard-changed", "write-error", "portal-error", "release-delayed", "gnome-portal-with-raw", "gnome-forced-raw", "gnome-cancel-start", "gnome-no-literal-backend", "gnome-cancel-raw-probe", "wtype-after-portal-denial", "targeted-no-screenshot"), default="paste")
    parser.add_argument("--backend", nargs=2, metavar=("STATE", "READY"), help=argparse.SUPPRESS)
    parser.add_argument("--inspect", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()

    if args.backend:
        return backend_main(Path(args.backend[0]), Path(args.backend[1]))
    if args.inspect:
        snapshot = inspect_backend(os.environ)
        print(json.dumps(snapshot, ensure_ascii=False))
        return 0
    if args.binary is None:
        parser.error("--binary is required")
    os.environ["PASTE_FIXTURE_SCENARIO"] = args.scenario
    binary = args.binary.expanduser().resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must be an executable file")
    return run(binary, args.save_evidence)


if __name__ == "__main__":
    raise SystemExit(main())
