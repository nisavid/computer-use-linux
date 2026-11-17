#!/usr/bin/env python3
"""Exercise KDE paste cancellation through stdio MCP and private D-Bus services."""

import contextlib
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tempfile
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib


PORTAL = "org.freedesktop.portal.Desktop"
DESKTOP = "/org/freedesktop/portal/desktop"
REMOTE = "org.freedesktop.portal.RemoteDesktop"
REQUEST = "org.freedesktop.portal.Request"
SESSION = "org.freedesktop.portal.Session"
KLIPPER = "org.kde.klipper"
CLIPBOARD = "org.kde.klipper.klipper"
CONTROL = "org.computeruse.PrivateFixture"
CONTROL_PATH = "/org/computeruse/PrivateFixture"

ORIGINAL = "PREVIOUS_CLIPBOARD"
PREPARED = "LITERAL_λ!"
INITIAL_FIELD = "LEFT_OLD_RIGHT"
PASTED_FIELD = "LEFT_" + PREPARED + "_RIGHT"
PASTE_KEYS = [["code", 29, 1], ["code", 47, 1], ["code", 47, 0], ["code", 29, 0]]
ESCAPE_KEYS = [["code", 1, 1], ["code", 1, 0]]
RAW_COMMANDS = {"xdotool", "ydotool", "wtype"}
SHIMS = RAW_COMMANDS | {"systemctl", "gdbus", "busctl", "ibus"}
START_CASES = {"cancel-start-success", "cancel-start-denied"}
CANCELLATION_CASES = START_CASES | {"cancel-prepared", "cancel-dispatched"}
PASTE_REPLY_TIMEOUT = "paste-reply-timeout"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def append_json(path, value):
    with path.open("a", encoding="utf-8") as stream:
        print(json.dumps(value, ensure_ascii=False), file=stream)


def wait_for(predicate, seconds, description):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = predicate()
        if result is not None:
            return result
        time.sleep(0.01)
    raise TimeoutError(description)


class State:
    def __init__(self, output, mode):
        self.output = output
        self.mode = mode
        self.pending_reply = None
        self.barrier_kind = None
        self.barrier_time = None
        self.clipboard = ORIGINAL
        self.field = INITIAL_FIELD
        self.keys = []
        self.held = set()
        self.sessions = {}
        self.requests = []
        self.record("fixture_started")

    def record(self, event, **fields):
        append_json(self.output / "dbus.jsonl", {
            "time": time.monotonic(), "event": event, **fields,
        })
        snapshot = {
            "clipboard": self.clipboard,
            "field": self.field,
            "keys": self.keys,
            "held": sorted(self.held),
            "pending": self.pending_reply is not None,
            "barrier_kind": self.barrier_kind,
            "barrier_time": self.barrier_time,
        }
        temporary = self.output / "snapshot.tmp"
        temporary.write_text(json.dumps(snapshot), encoding="utf-8")
        temporary.replace(self.output / "snapshot.json")

    def hold_reply(self, kind, reply, **fields):
        require(self.barrier_kind is None, "fixture barrier repeated")
        self.pending_reply = reply
        self.barrier_kind = kind
        self.barrier_time = time.monotonic()
        self.record(f"{kind}_held", **fields)

    def key(self, session, route, number, state):
        require(str(session) in self.sessions, "unknown portal session")
        number, state = int(number), int(state)
        identity = (route, number)
        if state == 1:
            self.held.add(identity)
            # The fixture models a physical Ctrl+V mapping for its own field.
            if identity == ("code", 47) and ("code", 29) in self.held:
                self.field = "LEFT_" + self.clipboard + "_RIGHT"
        else:
            self.held.discard(identity)
        self.keys.append([route, number, state])
        self.record("keyboard", session=str(session), route=route, number=number, state=state)


class RequestObject(dbus.service.Object):
    @dbus.service.signal(REQUEST, signature="ua{sv}")
    def Response(self, code, results):
        pass


class SessionObject(dbus.service.Object):
    @dbus.service.method(SESSION, in_signature="", out_signature="")
    def Close(self):
        pass


def portal_path(sender, kind, token):
    sender_component = sender.lstrip(":").replace(".", "_")
    return f"{DESKTOP}/{kind}/{sender_component}/{token}"


class Portal(dbus.service.Object):
    def __init__(self, name, state):
        super().__init__(name, DESKTOP)
        self.name = name
        self.state = state
        self.withheld_v_reply = None

    def respond(self, sender, options, results, *, start_code=None, hold=False):
        path = portal_path(sender, "request", options["handle_token"])
        request = RequestObject(self.name, path)
        self.state.requests.append(request)

        def send_response():
            if start_code is not None:
                self.state.record("start_response_sent", code=start_code, request=path)
            request.Response(dbus.UInt32(start_code or 0), dbus.Dictionary(results, signature="sv"))
            return GLib.SOURCE_REMOVE

        if hold:
            self.state.hold_reply("start_response", send_response, code=start_code, request=path)
        else:
            GLib.idle_add(send_response)
        return dbus.ObjectPath(path)

    @dbus.service.method(REMOTE, in_signature="a{sv}", out_signature="o", sender_keyword="sender")
    def CreateSession(self, options, sender):
        session = portal_path(sender, "session", options["session_handle_token"])
        self.state.sessions[session] = SessionObject(self.name, session)
        self.state.record("create_session", sender=sender, session=session)
        return self.respond(sender, options, {"session_handle": dbus.String(session, variant_level=1)})

    @dbus.service.method(REMOTE, in_signature="oa{sv}", out_signature="o", sender_keyword="sender")
    def SelectDevices(self, session, options, sender):
        require(str(session) in self.state.sessions, "unknown portal session")
        require(int(options["types"]) == 1, "expected keyboard-only device selection")
        return self.respond(sender, options, {})

    @dbus.service.method(REMOTE, in_signature="osa{sv}", out_signature="o", sender_keyword="sender")
    def Start(self, session, parent_window, options, sender):
        require(str(session) in self.state.sessions, "unknown portal session")
        self.state.record("start_session", session=str(session))
        hold = self.state.mode in START_CASES and self.state.barrier_kind is None
        code = 2 if hold and self.state.mode == "cancel-start-denied" else 0
        results = {} if code else {"devices": dbus.UInt32(1, variant_level=1)}
        return self.respond(sender, options, results, start_code=code, hold=hold)

    @dbus.service.method(
        REMOTE, in_signature="oa{sv}iu", out_signature="", async_callbacks=("reply", "error"),
    )
    def NotifyKeyboardKeycode(self, session, options, number, state, reply, error):
        self.state.key(session, "code", number, state)
        if self.state.mode == PASTE_REPLY_TIMEOUT and int(number) == 47 and int(state) == 1:
            # The key has already changed the synthetic field. Keep the
            # reply pending while continuing to serve release notifications.
            self.withheld_v_reply = reply
            self.state.record("paste_reply_withheld", session=str(session))
            return
        if (
            self.state.mode == "cancel-dispatched"
            and int(number) == 29
            and int(state) == 1
            and self.state.barrier_kind is None
        ):
            self.state.hold_reply("first_ctrl_reply", reply, session=str(session))
            return
        reply()

    @dbus.service.method(REMOTE, in_signature="oa{sv}iu", out_signature="")
    def NotifyKeyboardKeysym(self, session, options, number, state):
        self.state.key(session, "sym", number, state)


class Klipper(dbus.service.Object):
    def __init__(self, name, state):
        super().__init__(name, "/klipper")
        self.state = state

    @dbus.service.method(CLIPBOARD, in_signature="", out_signature="s")
    def getClipboardContents(self):
        self.state.record("clipboard_read", value=self.state.clipboard)
        return self.state.clipboard

    @dbus.service.method(
        CLIPBOARD, in_signature="s", out_signature="", async_callbacks=("reply", "error"),
    )
    def setClipboardContents(self, value, reply, error):
        self.state.clipboard = str(value)
        if self.state.mode == "cancel-prepared" and value == PREPARED:
            self.state.hold_reply("prepared_reply", reply, value=str(value))
            return
        self.state.record("clipboard_write", value=str(value))
        reply()


class Control(dbus.service.Object):
    def __init__(self, name, state):
        super().__init__(name, CONTROL_PATH)
        self.state = state

    @dbus.service.method(CONTROL, in_signature="", out_signature="")
    def ReleaseBarrier(self):
        reply = self.state.pending_reply
        require(reply is not None, "fixture reply is not pending")
        self.state.pending_reply = None
        self.state.record(f"{self.state.barrier_kind}_released")
        reply()


def serve(address, output, mode):
    DBusGMainLoop(set_as_default=True)
    bus = dbus.bus.BusConnection(address)
    state = State(output, mode)
    objects = [
        cls(dbus.service.BusName(name, bus=bus, do_not_queue=True), state)
        for cls, name in [(Portal, PORTAL), (Klipper, KLIPPER), (Control, CONTROL)]
    ]
    state.record("fixture_ready", owner=bus.get_unique_name())
    (output / "ready").touch()
    GLib.MainLoop().run()
    del objects


@contextlib.contextmanager
def child(command, environment, stderr, *, mcp=False):
    with stderr.open("wb") as error:
        process = subprocess.Popen(
            command, env=environment, stderr=error,
            stdin=subprocess.PIPE if mcp else subprocess.DEVNULL,
            stdout=subprocess.PIPE if mcp else subprocess.DEVNULL,
        )
        try:
            yield process
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)
            if mcp:
                process.stdin.close()
                process.stdout.close()
            stderr.with_suffix(".exit").write_text(f"{process.returncode}\n")


class Client:
    def __init__(self, process, output):
        self.process = process
        self.output = output
        self.buffer = b""
        self.inbox = {}

    def send(self, **message):
        frame = json.dumps({"jsonrpc": "2.0", **message}, ensure_ascii=False)
        wire = frame.encode("utf-8") + b"\n"
        with (self.output / "mcp.stdin").open("ab") as stream:
            stream.write(wire)
        append_json(self.output / "client.jsonl", {
            "time": time.monotonic(), "event": "send", "message": message,
        })
        self.process.stdin.write(wire)
        self.process.stdin.flush()

    def receive(self, request_id, deadline):
        while request_id not in self.inbox:
            if b"\n" not in self.buffer:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([self.process.stdout], [], [], remaining)[0]:
                    raise TimeoutError(f"MCP response {request_id}")
                chunk = os.read(self.process.stdout.fileno(), 65536)
                require(chunk, f"MCP stdout closed while waiting for {request_id}")
                with (self.output / "mcp.stdout").open("ab") as stream:
                    stream.write(chunk)
                self.buffer += chunk
                continue
            line, self.buffer = self.buffer.split(b"\n", 1)
            frame = json.loads(line)
            append_json(self.output / "client.jsonl", {
                "time": time.monotonic(), "event": "receive", "message": frame,
            })
            if "id" in frame:
                self.inbox[frame["id"]] = frame
        return self.inbox.pop(request_id)

    def tool(self, request_id, name, **arguments):
        self.send(id=request_id, method="tools/call", params={"name": name, "arguments": arguments})


def action_payload(frame):
    result = frame.get("result", {})
    if result.get("isError"):
        return None
    action = result.get("structuredContent")
    if action is None:
        for item in result.get("content", []):
            if item.get("type") == "text":
                action = json.loads(item["text"])
                break
    return action


def action_ok(frame):
    action = action_payload(frame)
    return isinstance(action, dict) and action.get("ok") is True


def snapshot(output):
    return json.loads((output / "snapshot.json").read_text(encoding="utf-8"))


def fixture_events(output):
    return [
        json.loads(line)
        for line in (output / "dbus.jsonl").read_text(encoding="utf-8").splitlines()
    ]


def cancel_at_barrier(client, control, output, mode):
    accepted = wait_for(
        lambda: snapshot(output)["barrier_time"], 8, f"{mode} barrier was not held",
    )
    gate_deadline = accepted + 2
    before = snapshot(output)
    require(before["pending"] and before["field"] == INITIAL_FIELD, "invalid barrier state")
    if mode == "cancel-dispatched":
        require(
            before["barrier_kind"] == "first_ctrl_reply"
            and before["clipboard"] == PREPARED
            and before["keys"] == PASTE_KEYS[:1]
            and before["held"] == [["code", 29]],
            "expected exactly the first Ctrl press at the barrier",
        )
    else:
        expected_kind, expected_clipboard = (
            ("prepared_reply", PREPARED) if mode == "cancel-prepared" else ("start_response", ORIGINAL)
        )
        require(
            before["barrier_kind"] == expected_kind
            and before["clipboard"] == expected_clipboard
            and not before["keys"]
            and not before["held"],
            "expected a held barrier before any keyboard dispatch",
        )
    client.send(method="notifications/cancelled", params={"requestId": 2})
    if mode == "cancel-dispatched":
        # Queue the next public input while the first operation still owns cleanup.
        client.tool(4, "press_key", key="Escape")
    client.send(id=3, method="ping")
    pong = client.receive(3, gate_deadline)
    require("result" in pong, "cancellation ordering ping failed")
    held = snapshot(output)
    require(held["pending"] and held["keys"] == before["keys"], "barrier changed before release")
    remaining = gate_deadline - time.monotonic()
    require(remaining > 0, "fixture reply barrier expired")
    control.ReleaseBarrier(timeout=remaining)
    require(time.monotonic() < gate_deadline, "fixture reply release missed barrier budget")


def check_paste_effect(events, after_probe):
    require(
        after_probe["field"] == PASTED_FIELD and after_probe["clipboard"] == ORIGINAL,
        "fixture paste effect or clipboard restoration missing",
    )
    writes = [(index, event["value"]) for index, event in enumerate(events)
              if event["event"] == "clipboard_write"]
    require([value for _, value in writes] == [PREPARED, ORIGINAL], "paste write or restore repeated")
    escape = next(
        index for index, event in enumerate(events)
        if event["event"] == "keyboard" and event["route"] == "code"
        and event["number"] == 1 and event["state"] == 1
    )
    require(writes[-1][0] < escape, "queued Escape dispatched before clipboard restoration")


def fixture_environment(root, output):
    environment = {
        "PATH": str(root / "bin"),
        "LANG": "C.UTF-8",
        "HOME": str(root / "home"),
        "TMPDIR": str(root / "tmp"),
        "XDG_CONFIG_HOME": str(root / "config"),
        "XDG_CACHE_HOME": str(root / "cache"),
        "XDG_DATA_HOME": str(root / "data"),
        "XDG_STATE_HOME": str(root / "state"),
        "XDG_RUNTIME_DIR": str(root / "runtime"),
        "XDG_CURRENT_DESKTOP": "KDE",
        "XDG_SESSION_DESKTOP": "KDE",
        "DESKTOP_SESSION": "KDE",
        "XDG_SESSION_TYPE": "wayland",
        "DISPLAY": ":98765",
        "WAYLAND_DISPLAY": "fixture-no-wayland",
        "XAUTHORITY": str(root / "runtime" / "no-xauthority"),
        "HYPRLAND_INSTANCE_SIGNATURE": "fixture-no-hyprland",
        "YDOTOOL_SOCKET": str(root / "runtime" / "no-ydotool"),
        "DBUS_SYSTEM_BUS_ADDRESS": f"unix:path={root}/runtime/no-system-bus",
        "DBUS_SESSION_BUS_ADDRESS": f"unix:path={root}/runtime/bus",
        "COMPUTER_USE_LINUX_FORCE_PORTAL_KEYBOARD": "1",
        "CU_DISABLE_ABS_POINTER": "1",
        "FIXTURE_DENIALS": str(output / "denied.jsonl"),
    }
    for directory in ("bin", "home", "tmp", "config", "cache", "data", "state", "runtime"):
        (root / directory).mkdir(mode=0o700)
    for name in sorted(SHIMS):
        shim = root / "bin" / name
        shim.write_text(
            "#!/usr/bin/python3\n"
            "import json, os, sys\n"
            "with open(os.environ['FIXTURE_DENIALS'], 'a') as stream:\n"
            "    print(json.dumps({'command': os.path.basename(sys.argv[0]),"
            " 'arguments': sys.argv[1:]}), file=stream)\n"
            "sys.exit(125)\n",
            encoding="utf-8",
        )
        shim.chmod(0o700)
        require(shutil.which(name, path=environment["PATH"]) == str(shim), f"wrong {name} shim")
        probe = subprocess.run([str(shim), "--fixture-preflight"], env=environment, timeout=2)
        require(probe.returncode == 125, f"{name} shim failed closed preflight")
    (output / "denied.jsonl").rename(output / "deny-preflight.jsonl")
    return environment


def bus_config(root):
    path = root / "dbus.conf"
    path.write_text(f"""<busconfig>
  <type>session</type>
  <listen>unix:path={root}/runtime/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow own="*"/>
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
  </policy>
</busconfig>
""", encoding="utf-8")
    return path


def scenario(binary, output, mode):
    output.mkdir(mode=0o700)
    with tempfile.TemporaryDirectory(prefix="cul-paste-", dir="/tmp") as directory:
        root = Path(directory)
        environment = fixture_environment(root, output)
        address = environment["DBUS_SESSION_BUS_ADDRESS"]
        config = bus_config(root)
        shutil.copyfile(config, output / "dbus.conf")
        daemon = ["/usr/bin/dbus-daemon", "--nofork", "--nopidfile", f"--config-file={config}"]
        service = [sys.executable, "-I", "-B", __file__, "--serve", address, str(output), mode]
        with child(daemon, environment, output / "dbus.stderr") as bus_process:
            wait_for(
                lambda: True if (root / "runtime/bus").is_socket() else None,
                3, "private bus socket unavailable",
            )
            require(bus_process.poll() is None, "private bus exited")
            with child(service, environment, output / "service.stderr") as service_process:
                wait_for(
                    lambda: True if (output / "ready").exists() else None,
                    3, "fake services did not register",
                )
                require(service_process.poll() is None, "fake services exited")
                bus = dbus.bus.BusConnection(address)
                owners = {name: str(bus.get_name_owner(name)) for name in (PORTAL, KLIPPER, CONTROL)}
                require(len(set(owners.values())) == 1, "unexpected fixture owners")
                append_json(output / "owners.jsonl", owners)
                control = dbus.Interface(
                    bus.get_object(CONTROL, CONTROL_PATH, introspect=False), CONTROL,
                )
                with child([binary, "mcp"], environment, output / "mcp.stderr", mcp=True) as mcp:
                    client = Client(mcp, output)
                    client.send(id=1, method="initialize", params={
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name": "kde-paste-cancellation-fixture", "version": "1"},
                    })
                    initialized = client.receive(1, time.monotonic() + 5)
                    require(initialized.get("result", {}).get("protocolVersion"), "initialize failed")
                    client.send(method="notifications/initialized")
                    client.tool(2, "type_text", text=PREPARED)
                    if mode in CANCELLATION_CASES:
                        cancel_at_barrier(client, control, output, mode)
                    # A cancelled request may have no response; observe cleanup and
                    # serialization through the fixture and a subsequent request.
                    typing = None if mode in CANCELLATION_CASES else client.receive(2, time.monotonic() + 10)
                    wait_for(
                        lambda: True if snapshot(output)["clipboard"] == ORIGINAL else None,
                        5, "clipboard was not restored",
                    )
                    before_probe = snapshot(output)
                    require(not before_probe["pending"], "fixture reply still pending")
                    if mode == "cancel-dispatched":
                        # The queued probe may have run by the time we read the typing reply.
                        expected_keys = PASTE_KEYS + ESCAPE_KEYS
                    else:
                        require(not before_probe["held"], "keys remained held after typing")
                        if mode == PASTE_REPLY_TIMEOUT:
                            require(before_probe["keys"] == PASTE_KEYS, "expected one chord and reverse releases")
                        expected_keys = before_probe["keys"] + ESCAPE_KEYS
                        client.tool(4, "press_key", key="Escape")
                    require(action_ok(client.receive(4, time.monotonic() + 8)), "input lock probe failed")
                    after_probe = snapshot(output)
                    require(
                        after_probe["keys"] == expected_keys and not after_probe["held"],
                        "unexpected serialization probe effects",
                    )

    denials = [
        json.loads(line)["command"]
        for line in (output / "denied.jsonl").read_text(encoding="utf-8").splitlines()
    ]
    require("systemctl" in denials, "expected environment hydration shim observation")
    require(not RAW_COMMANDS.intersection(denials), f"raw command attempted: {denials}")
    if mode == "control":
        require(
            action_ok(typing)
            and before_probe["keys"] == PASTE_KEYS
            and before_probe["field"] == PASTED_FIELD,
            "uncancelled route did not dispatch one constructed paste",
        )
        return 0
    if mode == "cancel-dispatched":
        check_paste_effect(fixture_events(output), after_probe)
        return 0
    if mode == PASTE_REPLY_TIMEOUT:
        events = fixture_events(output)
        require(
            sum(event["event"] == "paste_reply_withheld" for event in events) == 1,
            "expected one withheld V reply",
        )
        check_paste_effect(events, after_probe)
        action = action_payload(typing)
        require(isinstance(action, dict) and action.get("ok") is False, "expected a typing error")
        message = action.get("message", "")
        require("RemoteDesktop NotifyKeyboardKeycode timed out" in message, "missing portal timeout")
        if "Some or all paste input may have landed. Inspect the target before retrying." not in message:
            print("RED: applied_paste_error_omitted_uncertainty", file=sys.stderr)
            return 1
        return 0
    if mode in START_CASES:
        start_codes = [
            event["code"] for event in fixture_events(output)
            if event["event"] == "start_response_sent"
        ]
        expected = [2, 0] if mode == "cancel-start-denied" else [0]
        require(start_codes[:len(expected)] == expected, "unexpected portal Start responses")
    if before_probe["keys"]:
        print("RED: cancelled_type_text_dispatched_keyboard", file=sys.stderr)
        return 1
    require(
        before_probe["field"] == INITIAL_FIELD,
        "cancelled request changed its target",
    )
    return 0


def main():
    os.umask(0o077)
    if sys.argv[1] == "--serve":
        _, _, address, output, mode = sys.argv
        serve(address, Path(output), mode)
        return 0
    _, binary, output, host_mount, host_pid, host_net, *selection = sys.argv
    require(len(selection) <= 1, "select one cancellation case")
    mode = selection[0] if selection else "cancel-prepared"
    require(mode in CANCELLATION_CASES | {PASTE_REPLY_TIMEOUT}, f"unknown paste case {mode}")
    for kind, host in [("mnt", host_mount), ("pid", host_pid), ("net", host_net)]:
        actual = os.readlink(f"/proc/self/ns/{kind}")
        require(actual != host, f"fixture shares host {kind} namespace")
        append_json(Path(output) / "namespaces.jsonl", {"kind": kind, "host": host, "fixture": actual})
    require(not list(Path("/run").iterdir()), "fixture /run is not empty")
    for endpoint in ("/dev/input", "/dev/uinput", "/tmp/.X11-unix", "/var/run"):
        require(not Path(endpoint).exists(), f"unexpected endpoint {endpoint}")
    scenario(binary, Path(output) / "control", "control")
    return scenario(binary, Path(output) / mode, mode)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"FIXTURE ERROR: {error}", file=sys.stderr)
        sys.exit(2)
