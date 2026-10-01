#!/usr/bin/env python3
"""Public MCP input must not request screenshots as a geometry side effect.

Run with --binary pointing at a built server. Requires Linux user namespaces,
bubblewrap, dbus-daemon, Python 3, and PyGObject. Each case uses a new server,
private D-Bus services, a synthetic display/layout, recording command adapters,
and private HOME, /proc, /dev, /run, /tmp, and network. No GUI is started.

The fixture models target, key, pointer, and window effects at external backend
interfaces. It qualifies routing and observation boundaries, not real desktop
insertion, compositor coordinates, uinput, or toolkit behavior. Screenshot
requests are recorded before permission is decided: denial still fails the
zero-request contract for non-screenshot actions.
"""

import argparse
import base64
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import zlib

WINDOW = 'dev.avifenesh.ComputerUseLinux.WindowControl'
WINDOW_PATH = '/dev/avifenesh/ComputerUseLinux/WindowControl'
PORTAL = 'org.freedesktop.portal.Desktop'
PORTAL_PATH = '/org/freedesktop/portal/desktop'
SCREENSHOT = 'org.freedesktop.portal.Screenshot'
REMOTE = 'org.freedesktop.portal.RemoteDesktop'
SCREENCAST = 'org.freedesktop.portal.ScreenCast'
REQUEST = 'org.freedesktop.portal.Request'
SESSION = 'org.freedesktop.portal.Session'
TARGET = 101
DECOY = 202
CASES = (
    'cold-press-key', 'cold-move', 'cold-resize', 'cold-click', 'cold-scroll',
    'cold-relative-click', 'cold-relative-scroll', 'cold-center-scroll',
    'cold-portal-drag', 'warm-click', 'warm-relative-click',
    'warm-relative-scroll', 'warm-center-scroll', 'warm-portal-drag',
    'warm-offscreen-feedback', 'explicit-screenshot', 'explicit-target-refusal',
    'cold-press-key-command', 'cold-click-command', 'explicit-screenshot-command',
)


def interface_xml(name, methods, properties=()):
    xml = '<interface name="' + name + '">'
    for method, inputs, outputs in methods:
        xml += '<method name="' + method + '">'
        for signature in inputs:
            xml += '<arg type="' + signature + '" direction="in"/>'
        for signature in outputs:
            xml += '<arg type="' + signature + '" direction="out"/>'
        xml += '</method>'
    for name, signature in properties:
        xml += '<property name="' + name + '" type="' + signature + '" access="read"/>'
    return xml + '</interface>'


XML = '<node>' + interface_xml(WINDOW, (
    ('ListWindows', (), ('s',)), ('ActivateWindow', ('t',), ('b', 's')),
    ('MoveWindow', ('t', 'i', 'i'), ('b', 's')),
    ('ResizeWindow', ('t', 'i', 'i'), ('b', 's')),
    ('GetMonitorLayout', (), ('s',)),
)) + interface_xml(SCREENSHOT, (
    ('Screenshot', ('s', 'a{sv}'), ('o',)),
), (('version', 'u'),)) + interface_xml(REMOTE, (
    ('CreateSession', ('a{sv}',), ('o',)),
    ('SelectDevices', ('o', 'a{sv}'), ('o',)),
    ('Start', ('o', 's', 'a{sv}'), ('o',)),
    ('NotifyKeyboardKeysym', ('o', 'a{sv}', 'i', 'u'), ()),
    ('NotifyKeyboardKeycode', ('o', 'a{sv}', 'i', 'u'), ()),
    ('NotifyPointerMotionAbsolute', ('o', 'a{sv}', 'u', 'd', 'd'), ()),
    ('NotifyPointerButton', ('o', 'a{sv}', 'i', 'u'), ()),
    ('NotifyPointerAxisDiscrete', ('o', 'a{sv}', 'u', 'i'), ()),
), (('AvailableDeviceTypes', 'u'), ('version', 'u'))) + interface_xml(SCREENCAST, (
    ('SelectSources', ('o', 'a{sv}'), ('o',)),
), (('AvailableSourceTypes', 'u'), ('AvailableCursorModes', 'u'), ('version', 'u'))) + (
    '<interface name="' + REQUEST + '"><signal name="Response">'
    '<arg type="u"/><arg type="a{sv}"/></signal></interface>'
) + interface_xml(SESSION, (('Close', (), ()),)) + '</node>'


def gio():
    import gi
    from gi.repository import Gio, GLib
    return Gio, GLib


def write_json(path, value):
    pending = path.with_suffix('.pending')
    pending.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')
    os.replace(pending, path)


def png(width, height):
    def chunk(kind, data):
        return struct.pack('!I', len(data)) + kind + data + struct.pack('!I', zlib.crc32(kind + data))
    pixels = (b'\0' + b'\x35\x79\xb2' * width) * height
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('!IIBBBBB', width, height, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(pixels)) + chunk(b'IEND', b''))


class Fixture:
    def __init__(self, address, root):
        Gio, GLib = gio()
        self.connection = Gio.DBusConnection.new_for_address_sync(
            address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
            | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
        self.info = Gio.DBusNodeInfo.new_for_xml(XML)
        self.root = root
        self.lock = threading.RLock()
        self.events = []
        self.registrations = []
        self.sessions = {}
        self.monitors_available = False
        self.allow_screenshot = False
        self.pointer = [0, 0]
        self.button_down = False
        self.drag_start = None
        self.drags = []
        self.keys = {TARGET: [], DECOY: []}
        self.clicks = 0
        self.axes = []
        self.windows = [dict(window_id=wid, title=title, app_id=app, wm_class=None,
            pid=os.getpid(), bounds=dict(x=x, y=30, width=200, height=100),
            workspace=0, focused=wid == DECOY, hidden=False, client_type='wayland',
            backend='gnome-shell-extension') for wid, title, app, x in (
                (TARGET, 'Input observation target', 'fixture-target', 40),
                (DECOY, 'Input observation decoy', 'fixture-decoy', 350))]
        for name in (WINDOW, PORTAL):
            result = self.connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                'org.freedesktop.DBus', 'RequestName', GLib.Variant('(su)', (name, 4)),
                None, Gio.DBusCallFlags.NONE, 2000, None)
            assert result.unpack()[0] == 1, ('fixture name ownership failed', name)
        self.export(WINDOW_PATH, (WINDOW,))
        self.export(PORTAL_PATH, (SCREENSHOT, REMOTE, SCREENCAST))

    def export(self, path, interfaces):
        for interface in interfaces:
            self.registrations.append(self.connection.register_object(path,
                self.info.lookup_interface(interface), self.method, self.property, None))

    def property(self, connection, sender, path, interface, name):
        _, GLib = gio()
        values = {'version': 2, 'AvailableDeviceTypes': 3,
                  'AvailableSourceTypes': 1, 'AvailableCursorModes': 1}
        return GLib.Variant('u', values[name]) if name in values else None

    def record(self, event, **fields):
        self.events.append(dict(event=event, **fields))

    def response(self, sender, options, code=0, values=None):
        _, GLib = gio()
        token = options['handle_token']
        request = '/org/freedesktop/portal/desktop/request/' + sender.lstrip(':').replace('.', '_') + '/' + token
        self.export(request, (REQUEST,))

        def emit():
            self.connection.emit_signal(None, request, REQUEST, 'Response',
                GLib.Variant('(ua{sv})', (code, values or {})))
            return False

        GLib.timeout_add(40, emit)
        return GLib.Variant('(o)', (request,))

    def method(self, connection, sender, path, interface, method, parameters, invocation):
        _, GLib = gio()
        args = parameters.unpack()
        try:
            with self.lock:
                if method == 'ListWindows':
                    result = GLib.Variant('(s)', (json.dumps(self.windows),))
                elif method == 'GetMonitorLayout':
                    self.record(method, available=self.monitors_available)
                    if not self.monitors_available:
                        invocation.return_dbus_error('org.freedesktop.DBus.Error.UnknownMethod', 'fixture monitor geometry unavailable')
                        return
                    result = GLib.Variant('(s)', (json.dumps([dict(index=0, x=0, y=0,
                        width=640, height=480, primary=True, scale=1.0)]),))
                elif method in ('ActivateWindow', 'MoveWindow', 'ResizeWindow'):
                    window = next((w for w in self.windows if w['window_id'] == args[0]), None)
                    if window is None:
                        result = GLib.Variant('(bs)', (False, 'unknown constructed window'))
                    else:
                        if method == 'ActivateWindow':
                            for item in self.windows:
                                item['focused'] = item is window
                        elif method == 'MoveWindow':
                            window['bounds'].update(x=args[1], y=args[2])
                        else:
                            window['bounds'].update(width=args[1], height=args[2])
                        self.record(method, arguments=list(args))
                        result = GLib.Variant('(bs)', (True, 'constructed window operation applied'))
                elif method == 'Screenshot':
                    self.record(method, parent=args[0], options=args[1], allowed=self.allow_screenshot,
                                sender=sender)
                    values = {'uri': GLib.Variant('s', (self.root / 'screen.png').as_uri())} if self.allow_screenshot else {}
                    result = self.response(sender, args[1], 0 if self.allow_screenshot else 2, values)
                elif method == 'CreateSession':
                    options = args[0]
                    session = '/org/freedesktop/portal/desktop/session/' + sender.lstrip(':').replace('.', '_') + '/' + options['session_handle_token']
                    self.sessions[session] = 0
                    self.export(session, (SESSION,))
                    self.record(method, session=session)
                    result = self.response(sender, options, values={'session_handle': GLib.Variant('s', session)})
                elif method == 'SelectDevices':
                    self.sessions[args[0]] = args[1]['types']
                    self.record(method, types=args[1]['types'])
                    result = self.response(sender, args[1])
                elif method == 'SelectSources':
                    self.record(method, options=args[1])
                    result = self.response(sender, args[1])
                elif method == 'Start':
                    devices = self.sessions[args[0]]
                    self.record(method, devices=devices)
                    values = {'devices': GLib.Variant('u', devices)}
                    if devices & 2:
                        values['streams'] = GLib.Variant('a(ua{sv})', [(17, {
                            'position': GLib.Variant('(ii)', (0, 0)),
                            'size': GLib.Variant('(ii)', (640, 480))})])
                    result = self.response(sender, args[2], values=values)
                elif method in ('NotifyKeyboardKeysym', 'NotifyKeyboardKeycode'):
                    focused = next(w['window_id'] for w in self.windows if w['focused'])
                    self.keys[focused].append([method, args[2], args[3]])
                    self.record(method, window_id=focused, key=args[2], state=args[3])
                    result = GLib.Variant('()', ())
                elif method == 'NotifyPointerMotionAbsolute':
                    assert args[2] == 17, ('unexpected constructed stream', args)
                    self.pointer = [args[3], args[4]]
                    self.record(method, point=self.pointer.copy())
                    result = GLib.Variant('()', ())
                elif method == 'NotifyPointerButton':
                    assert args[2] == 272, ('unexpected pointer button', args)
                    if args[3]:
                        self.button_down = True
                        self.drag_start = self.pointer.copy()
                    else:
                        assert self.button_down, 'pointer release without press'
                        self.button_down = False
                        self.clicks += 1
                        if self.drag_start != self.pointer:
                            self.drags.append([self.drag_start, self.pointer.copy()])
                    self.record(method, button=args[2], state=args[3])
                    result = GLib.Variant('()', ())
                elif method == 'NotifyPointerAxisDiscrete':
                    self.axes.append([args[2], args[3]])
                    self.record(method, axis=args[2], steps=args[3])
                    result = GLib.Variant('()', ())
                elif method == 'Close':
                    self.record(method, session=path)
                    result = GLib.Variant('()', ())
                else:
                    invocation.return_dbus_error('org.freedesktop.DBus.Error.UnknownMethod', method)
                    return
            invocation.return_value(result)
        except Exception:
            self.record('fixture_error', method=method, traceback=traceback.format_exc())
            invocation.return_dbus_error('org.example.InputObservationFixture.Error', 'constructed backend rejected invalid request')

    def snapshot(self):
        with self.lock:
            return json.loads(json.dumps(dict(events=self.events, windows=self.windows,
                keys=self.keys, pointer=self.pointer, button_down=self.button_down,
                drags=self.drags, clicks=self.clicks, axes=self.axes)))


COMMAND_ADAPTER = '''#!/usr/bin/python3
import fcntl, json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
if name in ('gnome-screenshot', 'grim', 'spectacle'):
    # Availability/version queries don't request pixels. Keep the attempt
    # counter on capture invocations, including failed or denied captures.
    if args in (['--version'], ['--help'], ['-h']):
        print(name + ' private fixture metadata')
        sys.exit(0)
    path = Path(os.environ['OBSERVATION_COMMAND_STATE'])
    allowed = (name == 'gnome-screenshot' and os.environ['OBSERVATION_ALLOW_COMMAND_SCREENSHOT'] == '1'
               and len(args) == 2 and args[0] == '-f')
    with open(str(path) + '.lock', 'a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = json.loads(path.read_text())
        state['captures'].append(dict(command=name, argv=args, allowed=allowed))
        path.write_text(json.dumps(state))
    if not allowed:
        sys.exit(1)
    destination = Path(args[1]).resolve()
    if not destination.is_relative_to('/tmp'):
        sys.exit(125)
    destination.write_bytes(Path(os.environ['OBSERVATION_SCREENSHOT_PNG']).read_bytes())
    sys.exit(0)
if name == 'xrandr':
    if args == ['--listactivemonitors']:
        print('Monitors: 1\\n 0: +*FIXTURE 640/170x480/130+0+0 FIXTURE')
        sys.exit(0)
    sys.exit(1)
if name != 'ydotool':
    sys.exit(1)
if args and args[0] in ('help', '--help'):
    print('Usage: ydotool <cmd> <args>\\nAvailable commands:\\n  click\\n  mousemove\\n  type\\n  key\\n  debug')
    sys.exit(0)
# Capability qualification runs with its own private socket and removes
# XDG_RUNTIME_DIR. It must not count as application dispatch.
if 'XDG_RUNTIME_DIR' not in os.environ:
    sys.exit(0)
path = Path(os.environ['OBSERVATION_COMMAND_STATE'])
with open(str(path) + '.lock', 'a') as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    state = json.loads(path.read_text())
    state['commands'].append(args)
    point = state['pointer']
    hit = ('target' if 40 <= point[0] < 240 and 30 <= point[1] < 130 else
           'decoy' if 350 <= point[0] < 550 and 30 <= point[1] < 130 else 'outside')
    if len(args) == 5 and args[:3] == ['mousemove', '--absolute', '--']:
        state['pointer'] = [int(args[3]), int(args[4])]
    elif len(args) == 5 and args[:3] == ['mousemove', '--wheel', '--']:
        state['wheel'].append([int(args[3]), int(args[4])])
        state[hit + '_wheel'].append([int(args[3]), int(args[4])])
    elif args and args[0] == 'click' and args[-1] == '0xC0':
        state['clicks'] += 1
        state[hit + '_clicks'] += 1
    else:
        state['unexpected'].append(args)
        path.write_text(json.dumps(state))
        sys.exit(1)
    path.write_text(json.dumps(state))
'''


class Mcp:
    def __init__(self, process, report):
        self.process = process
        self.sequence = 0
        self.report = report
        self.rpc('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
                               'clientInfo': {'name': 'input-observation-fixture', 'version': '1'}})
        self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})

    def send(self, request):
        self.process.stdin.write(json.dumps(request) + '\n')
        self.process.stdin.flush()

    def rpc(self, method, params):
        self.sequence += 1
        request = {'jsonrpc': '2.0', 'id': self.sequence, 'method': method, 'params': params}
        self.send(request)
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            if not select.select([self.process.stdout], [], [], .2)[0]:
                continue
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError('server exited before MCP response')
            response = json.loads(line)
            if response.get('id') == self.sequence:
                self.report['mcp'].append({'request': request, 'response': response})
                assert 'error' not in response, response
                assert not response['result'].get('isError'), response
                return response['result']
        raise TimeoutError('MCP call did not finish within the fixture deadline')

    def call(self, name, arguments):
        result = self.rpc('tools/call', {'name': name, 'arguments': arguments})
        payload = result.get('structuredContent')
        if payload is None:
            payload = json.loads(next(c['text'] for c in result['content'] if c['type'] == 'text'))
        return payload, result


def stop(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=3)
    # A launcher may have exited before a descendant in its owned group.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def private_case(binary, case, output):
    Gio, GLib = gio()
    assert os.getpid() == 2 and os.getppid() == 1, 'private mode needs the bwrap PID namespace'
    assert not Path('/dev/uinput').exists() and not Path('/dev/input').exists(), 'live devices must be absent'
    assert os.environ.get('HOME') == '/home/fixture', 'private HOME missing'
    Path('/etc/machine-id').write_text('013579bdf02468ace013579bdf02468ac\n')
    # EXTERNAL D-Bus authentication resolves the peer UID through NSS. Supply
    # synthetic records in private /etc, without mounting the host account DB.
    Path('/etc/passwd').write_text('fixture:x:' + str(os.getuid()) + ':' + str(os.getgid())
        + ':Input observation fixture:/home/fixture:/bin/false\n')
    Path('/etc/group').write_text('fixture:x:' + str(os.getgid()) + ':\n')
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.prctl(36, 1, 0, 0, 0) == 0, 'could not become child subreaper'
    root = Path(tempfile.mkdtemp(prefix='input-observation-'))
    for folder in ('bin', 'runtime', 'config', 'state', 'cache', 'data'):
        (root / folder).mkdir(mode=0o700)
    (root / 'screen.png').write_bytes(png(640, 480))
    command_state = root / 'commands.json'
    write_json(command_state, dict(commands=[], captures=[], pointer=[0, 0], clicks=0, wheel=[], unexpected=[],
        target_clicks=0, decoy_clicks=0, outside_clicks=0,
        target_wheel=[], decoy_wheel=[], outside_wheel=[]))
    for command in ('ydotool', 'xrandr', 'ydotoold', 'xdotool', 'wtype', 'systemctl', 'gsettings',
                    'wmctrl', 'xprop', 'hyprctl', 'niri', 'i3-msg', 'kscreen-doctor', 'swaymsg',
                    'computer-use-linux-cosmic', 'gnome-screenshot', 'grim', 'spectacle'):
        path = root / 'bin' / command
        path.write_text(COMMAND_ADAPTER)
        path.chmod(0o700)
    config = root / 'bus.conf'
    config.write_text('<busconfig><type>session</type><listen>unix:path=' + str(root / 'runtime' / 'bus')
        + '</listen><auth>EXTERNAL</auth><policy context="default"><allow own="*"/>'
          '<allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>')
    env = dict(os.environ, PATH=str(root / 'bin') + ':/usr/bin:/bin', LANG='C.UTF-8',
        XDG_CONFIG_HOME=str(root / 'config'), XDG_CACHE_HOME=str(root / 'cache'),
        XDG_STATE_HOME=str(root / 'state'), XDG_DATA_HOME=str(root / 'data'),
        XDG_RUNTIME_DIR=str(root / 'runtime'), DISPLAY=':65535',
        XAUTHORITY=str(root / 'no-xauthority'), WAYLAND_DISPLAY='fixture-no-display',
        XDG_SESSION_TYPE='wayland', XDG_CURRENT_DESKTOP='GNOME', DESKTOP_SESSION='gnome',
        XDG_SESSION_DESKTOP='gnome', HYPRLAND_INSTANCE_SIGNATURE='fixture-none',
        YDOTOOL_SOCKET=str(root / 'no-input.sock'),
        COMPUTER_USE_LINUX_SCREENSHOT_BACKEND='portal',
        COMPUTER_USE_LINUX_FORCE_PORTAL_KEYBOARD='1',
        OBSERVATION_COMMAND_STATE=str(command_state),
        OBSERVATION_ALLOW_COMMAND_SCREENSHOT='1' if case == 'explicit-screenshot-command' else '0',
        OBSERVATION_SCREENSHOT_PNG=str(root / 'screen.png'))
    if case.endswith('-command'):
        env['COMPUTER_USE_LINUX_SCREENSHOT_BACKEND'] = 'gnome-screenshot'
    # Keep absolute-pointer initialization enabled. Its /dev has no uinput;
    # this catches capture-before-opening-the-device, without any live input.
    if 'portal-drag' in case:
        env['COMPUTER_USE_LINUX_FORCE_PORTAL_POINTER'] = '1'
    else:
        env['COMPUTER_USE_LINUX_FORCE_YDOTOOL_POINTER'] = '1'
    report = dict(case=case, scope='synthetic public MCP/backend behavior only',
        binary_sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest(), mcp=[],
        isolation=dict(pid_namespace=True, private_proc=True, private_dev=True,
            private_home=True, private_bus=True, private_network=True, gui=False), passed=False)
    processes = []
    handles = []
    fixture = None
    loop = None
    thread = None
    error = None
    try:
        bus_log = open(output / 'bus.stderr', 'w')
        handles.append(bus_log)
        bus = subprocess.Popen(['/usr/bin/dbus-daemon', '--config-file=' + str(config),
            '--nofork', '--nopidfile', '--print-address=1'], env=env,
            stdout=subprocess.PIPE, stderr=bus_log, text=True, start_new_session=True)
        processes.append(bus)
        assert select.select([bus.stdout], [], [], 5)[0], 'private D-Bus did not announce an address'
        address = bus.stdout.readline().strip()
        assert address.startswith('unix:path=' + str(root / 'runtime' / 'bus')), address
        env['DBUS_SESSION_BUS_ADDRESS'] = address
        fixture = Fixture(address, root)
        loop = GLib.MainLoop()
        thread = threading.Thread(target=loop.run, daemon=True)
        thread.start()
        server_log = open(output / 'server.stderr', 'w')
        handles.append(server_log)
        server = subprocess.Popen([binary, 'mcp'], env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=server_log, text=True, start_new_session=True)
        processes.append(server)
        client = Mcp(server, report)
        state, _ = client.call('get_app_state', {'window_id': TARGET, 'include_screenshot': False})
        assert state.get('screenshot') is None, state
        assert state['window_context']['window_id'] == TARGET, state
        assert not any(e['event'] == 'Screenshot' for e in fixture.events), fixture.snapshot()
        assert json.loads(command_state.read_text())['captures'] == [], 'non-screenshot observation launched capture'
        action_case = case.removesuffix('-command')
        warm = case.startswith('warm-')
        if warm:
            fixture.allow_screenshot = True
            payload, result = client.call('screenshot', {})
            assert (payload['coordinate_width'], payload['coordinate_height']) == (640, 480), payload
            assert any(c['type'] == 'image' for c in result['content']), result
            fixture.allow_screenshot = False
            # Relative conversion needs independently supplied logical layout.
            # Point warnings and pointer initialization stay on the cold-query
            # fallback path in warm-click/offscreen-feedback instead.
            fixture.monitors_available = case not in ('warm-click', 'warm-offscreen-feedback')
        screenshots_before = len([e for e in fixture.events if e['event'] == 'Screenshot'])
        operation_events_before = len(fixture.events)
        if action_case in ('cold-press-key', 'warm-offscreen-feedback'):
            fixture.windows[0]['bounds']['x'] = -40
            payload, _ = client.call('press_key', {'window_id': TARGET, 'key': 'Enter'})
            assert payload['ok'], payload
            assert fixture.keys[TARGET] == [['NotifyKeyboardKeysym', 65293, 1],
                                            ['NotifyKeyboardKeysym', 65293, 0]], fixture.snapshot()
            assert fixture.keys[DECOY] == [], fixture.snapshot()
            assert ('WARNING:' in payload['message']) == warm, payload
            assert 'Focused-element feedback unavailable' in payload['message'], payload
        elif case in ('cold-move', 'cold-resize'):
            name, args = ('move_window', {'window_id': TARGET, 'x': -40, 'y': 35}) if case == 'cold-move' else (
                'resize_window', {'window_id': TARGET, 'width': 220, 'height': 110})
            if name == 'resize_window':
                fixture.windows[0]['bounds']['x'] = -40
            decoy_before = fixture.snapshot()['windows'][1]
            payload, _ = client.call(name, args)
            assert payload['ok'] and payload['window']['window_id'] == TARGET, payload
            expected = dict(x=-40, y=35, width=200, height=100) if name == 'move_window' else dict(x=-40, y=30, width=220, height=110)
            assert fixture.windows[0]['bounds'] == expected, fixture.snapshot()
            assert fixture.snapshot()['windows'][1] == decoy_before, fixture.snapshot()
            assert 'WARNING:' not in payload['message'], payload
        elif action_case in ('explicit-screenshot', 'explicit-target-refusal'):
            fixture.monitors_available = True
            fixture.allow_screenshot = True
            args = {'window_id': TARGET} if action_case == 'explicit-screenshot' else {'window_id': 99999}
            if case == 'explicit-target-refusal':
                # Screenshot failures use an MCP isError response, so inspect
                # the protocol directly without the success-only client helper.
                client.sequence += 1
                request = dict(jsonrpc='2.0', id=client.sequence, method='tools/call',
                    params=dict(name='screenshot', arguments=args))
                client.send(request)
                deadline = time.monotonic() + 25
                while time.monotonic() < deadline:
                    if select.select([server.stdout], [], [], .2)[0]:
                        response = json.loads(server.stdout.readline())
                        if response.get('id') == client.sequence:
                            report['mcp'].append(dict(request=request, response=response))
                            assert 'error' in response or response.get('result', {}).get('isError'), response
                            break
                else:
                    raise TimeoutError('targeted screenshot refusal did not finish')
            else:
                payload, result = client.call('screenshot', args)
                assert payload['cropped_to_window'] and payload['window_title'] == 'Input observation target', payload
                assert (payload['coordinate_width'], payload['coordinate_height']) == (200, 100), payload
                image = next(c for c in result['content'] if c['type'] == 'image')
                assert struct.unpack('!II', base64.b64decode(image['data'])[16:24]) == (200, 100), image
        else:
            relative = 'relative' in case
            center = 'center' in case
            if 'portal-drag' in case:
                name, args = 'drag', dict(start_x=47, start_y=39, end_x=70, end_y=80)
            elif 'scroll' in case:
                name, args = 'scroll', dict(window_id=TARGET, direction='down', pages=.2)
                if not center:
                    args.update(x=7 if relative else 47, y=9 if relative else 39)
                if relative:
                    args['relative'] = True
            else:
                name, args = 'click', dict(window_id=TARGET,
                    x=7 if relative else 47, y=9 if relative else 39)
                if relative:
                    args['relative'] = True
            payload, _ = client.call(name, args)
            commands = json.loads(command_state.read_text())
            safe_refusal = not warm and (relative or center or 'portal-drag' in case)
            assert payload['ok'] == (not safe_refusal), payload
            assert not commands['unexpected'], commands
            if safe_refusal:
                assert commands['commands'] == [] and fixture.clicks == 0 and fixture.axes == [] and fixture.drags == [], (commands, fixture.snapshot())
                assert 'coordinate' in payload['message'].lower() or 'screenshot dimensions' in payload['message'].lower(), payload
            elif 'portal-drag' in case:
                assert fixture.drags == [[[47.0, 39.0], [70.0, 80.0]]] and not fixture.button_down, fixture.snapshot()
                assert commands['commands'] == [], commands
            else:
                expected_point = [140, 80] if center else [47, 39]
                assert commands['pointer'] == expected_point, commands
                if name == 'click':
                    assert commands['clicks'] == 1 and commands['wheel'] == [], commands
                    assert commands['target_clicks'] == 1 and commands['decoy_clicks'] == 0 and commands['outside_clicks'] == 0, commands
                else:
                    assert commands['wheel'] == [[0, -1]] and commands['clicks'] == 0, commands
                    assert commands['target_wheel'] == [[0, -1]] and commands['decoy_wheel'] == [] and commands['outside_wheel'] == [], commands
                assert 'WARNING:' not in payload['message'], payload
            if name != 'drag':
                assert fixture.windows[0]['focused'] and not fixture.windows[1]['focused'], fixture.snapshot()
        screenshots = [e for e in fixture.events if e['event'] == 'Screenshot']
        expected_screenshots = screenshots_before + int(case == 'explicit-screenshot')
        assert len(screenshots) == expected_screenshots, (
            'incidental screenshot request, including denial',
            {'expected': expected_screenshots, 'observed': screenshots})
        assert all(e['allowed'] for e in screenshots), ('denied screenshot is still an observation attempt', screenshots)
        captures = json.loads(command_state.read_text())['captures']
        assert len(captures) == int(case == 'explicit-screenshot-command'), ('incidental capture command, including failure', captures)
        assert all(e['allowed'] for e in captures), ('failed command is still a capture attempt', captures)
        for event in screenshots:
            assert event['parent'] == '' and event['options']['interactive'] is False, event
            assert isinstance(event['options']['handle_token'], str) and event['options']['handle_token'], event
        action_events = fixture.events[operation_events_before:]
        assert not any(e['event'] == 'fixture_error' for e in action_events), action_events
        report['passed'] = True
    except Exception:
        error = traceback.format_exc()
        report['failure'] = error
    finally:
        if fixture is not None:
            report['backend'] = fixture.snapshot()
        report['commands'] = json.loads(command_state.read_text())
        for process in reversed(processes[1:]):
            stop(process)
        if loop is not None:
            loop.quit()
        if thread is not None:
            thread.join(timeout=3)
        if fixture is not None:
            fixture.connection.close_sync(None)
        if processes:
            stop(processes[0])
        deadline = time.monotonic() + 3
        reaped = []
        while True:
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                report['cleanup'] = dict(echild=True, reaped=reaped)
                break
            if pid:
                reaped.append(dict(pid=pid, status=status))
            elif time.monotonic() >= deadline:
                report['cleanup'] = dict(echild=False, reaped=reaped)
                report['passed'] = False
                break
            else:
                time.sleep(.05)
        for handle in handles:
            handle.close()
        write_json(output / 'result.json', report)
        print(json.dumps(dict(case=case, passed=report['passed'], cleanup=report['cleanup'])), flush=True)
    if error or not report['passed']:
        raise AssertionError(error or 'fixture subprocess cleanup incomplete')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--case', choices=('all',) + CASES, default='all')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--private', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.private:
        private_case(args.binary, args.case, args.output)
        return
    binary = Path(args.binary).resolve(strict=True)
    script = Path(__file__).resolve()
    if shutil.which('bwrap') is None or not Path('/usr/bin/dbus-daemon').exists():
        parser.error('requires bubblewrap and /usr/bin/dbus-daemon; no unisolated fallback is permitted')
    output = (args.output or Path(tempfile.mkdtemp(prefix='cul-input-observation-evidence-'))).resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=True)
    cases = CASES if args.case == 'all' else (args.case,)
    failures = []
    for case in cases:
        case_output = output / case
        case_output.mkdir(mode=0o700, exist_ok=False)
        command = ['bwrap', '--unshare-all', '--die-with-parent', '--new-session', '--clearenv',
            '--ro-bind', '/usr', '/usr', '--symlink', 'usr/bin', '/bin',
            '--symlink', 'usr/lib', '/lib', '--symlink', 'usr/lib', '/lib64',
            '--dir', '/etc', '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp',
            '--tmpfs', '/run', '--dir', '/home', '--dir', '/home/fixture',
            '--dir', '/fixture', '--ro-bind', str(binary), '/fixture/server',
            '--ro-bind', str(script), '/fixture/test.py', '--bind', str(case_output), '/evidence',
            '--setenv', 'PATH', '/usr/bin:/bin', '--setenv', 'LANG', 'C.UTF-8',
            '--setenv', 'HOME', '/home/fixture', '--chdir', '/home/fixture']
        for system_file in ('/etc/ld.so.cache', '/etc/os-release'):
            if Path(system_file).exists():
                command.extend(['--ro-bind', system_file, system_file])
        command.extend(['--', '/usr/bin/python3', '/fixture/test.py', '--binary', '/fixture/server',
            '--case', case, '--output', '/evidence', '--private'])
        with open(case_output / 'isolation.stdout', 'w') as stdout, open(case_output / 'isolation.stderr', 'w') as stderr:
            process = subprocess.Popen(command, env={'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'},
                stdout=stdout, stderr=stderr, start_new_session=True)
            try:
                code = process.wait(timeout=75)
            except subprocess.TimeoutExpired:
                stop(process)
                code = -1
            finally:
                stop(process)
        if code != 0:
            failures.append(dict(case=case, exit_code=code))
    summary = dict(scope='synthetic public MCP/backend behavior only', cases=list(cases),
        failures=failures, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
        harness_sha256=hashlib.sha256(script.read_bytes()).hexdigest())
    write_json(output / 'summary.json', summary)
    print(json.dumps(dict(evidence=str(output), cases=len(cases), failures=failures)), flush=True)
    if failures:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
