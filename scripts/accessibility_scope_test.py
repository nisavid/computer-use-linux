#!/usr/bin/env python3
"""Opt-in headless AT-SPI/MCP regression using disposable D-Bus services.

Run with a built computer-use-linux binary. Requires bubblewrap, dbus-daemon,
and PyGObject. The complete fixture runs in private PID/network/IPC namespaces
with private /proc, /tmp, /run, and /dev; it never queries the host desktop.
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import threading
import time

ACCESSIBLE = 'org.a11y.atspi.Accessible'
ACTION = 'org.a11y.atspi.Action'
EDITABLE = 'org.a11y.atspi.EditableText'
ROOT = '/org/a11y/atspi/accessible/root'
WINDOW_SERVICE = 'dev.avifenesh.ComputerUseLinux.WindowControl'
WINDOW_PATH = '/dev/avifenesh/ComputerUseLinux/WindowControl'
XML = '''<node>
<interface name="org.a11y.atspi.Accessible">
 <method name="GetChildAtIndex"><arg type="i" direction="in"/><arg type="(so)" direction="out"/></method>
 <method name="GetInterfaces"><arg type="as" direction="out"/></method>
 <method name="GetRoleName"><arg type="s" direction="out"/></method>
 <method name="GetRole"><arg type="u" direction="out"/></method>
 <method name="GetState"><arg type="au" direction="out"/></method>
 <property name="Name" type="s" access="read"/>
 <property name="Description" type="s" access="read"/>
 <property name="ChildCount" type="i" access="read"/>
</interface>
<interface name="org.a11y.atspi.Action">
 <method name="GetActions"><arg type="a(sss)" direction="out"/></method>
 <method name="DoAction"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
 <property name="NActions" type="i" access="read"/>
</interface>
<interface name="org.a11y.atspi.EditableText">
 <method name="SetTextContents"><arg type="s" direction="in"/><arg type="b" direction="out"/></method>
</interface>
<interface name="org.a11y.atspi.Component">
 <method name="GetExtents"><arg type="u" direction="in"/><arg type="(iiii)" direction="out"/></method>
</interface>
<interface name="org.example.ScopeFixture">
 <method name="ReadState"><arg type="s" direction="out"/></method>
 <method name="ReadProtocolState"><arg type="s" direction="out"/></method>
 <method name="HoldActions"><arg type="b" direction="in"/><arg type="b" direction="out"/></method>
</interface>
<interface name="org.a11y.Bus">
 <method name="GetAddress"><arg type="s" direction="out"/></method>
</interface>
<interface name="org.a11y.Status">
 <property name="IsEnabled" type="b" access="read"/>
 <property name="ScreenReaderEnabled" type="b" access="read"/>
</interface>
<interface name="dev.avifenesh.ComputerUseLinux.WindowControl">
 <method name="ListWindows"><arg type="s" direction="out"/></method>
 <method name="ActivateWindow"><arg type="t" direction="in"/><arg type="b" direction="out"/><arg type="s" direction="out"/></method>
</interface>
</node>'''


def gio():
    import gi
    from gi.repository import Gio, GLib
    return Gio, GLib


def connect(address):
    Gio, _ = gio()
    return Gio.DBusConnection.new_for_address_sync(
        address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
        | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)


def own_name(connection, name):
    Gio, GLib = gio()
    connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                         'org.freedesktop.DBus', 'RequestName',
                         GLib.Variant('(su)', (name, 0)), None,
                         Gio.DBusCallFlags.NONE, 2000, None)


class Service:
    def __init__(self, connection, address, label=None, roots=None, windows=None):
        self.connection = connection
        self.address = address
        self.label = label
        self.roots = roots or []
        self.windows = windows or []
        self.activations = 0
        self.text = 'original'
        self.registrations = []
        self.hold_addresses = False
        self.pending_addresses = []
        self.hold_actions = False
        self.pending_actions = []
        self.accessibility_reads = 0

    def export(self, path, interfaces):
        Gio, _ = gio()
        info = Gio.DBusNodeInfo.new_for_xml(XML)
        for interface in interfaces:
            self.registrations.append(self.connection.register_object(
                path, info.lookup_interface(interface), self.method,
                self.property, None))

    def children(self, path):
        if self.label is None:
            return self.roots
        if path == ROOT:
            return [(self.connection.get_unique_name(), ROOT + '/button'),
                    (self.connection.get_unique_name(), ROOT + '/entry')]
        return []

    def interfaces(self, path):
        if path.endswith('/button'):
            return [ACCESSIBLE, ACTION, 'org.a11y.atspi.Component']
        if path.endswith('/entry'):
            return [ACCESSIBLE, EDITABLE, 'org.a11y.atspi.Component']
        return [ACCESSIBLE]

    def method(self, connection, sender, path, interface, method, parameters, invocation):
        _, GLib = gio()
        args = parameters.unpack()
        if interface in (ACCESSIBLE, ACTION, EDITABLE, 'org.a11y.atspi.Component'):
            self.accessibility_reads += 1
        if method == 'GetChildAtIndex':
            children = self.children(path)
            if not 0 <= args[0] < len(children):
                invocation.return_dbus_error('org.a11y.atspi.Error', 'no child at index')
                return
            result = GLib.Variant('((so))', (children[args[0]],))
        elif method == 'GetInterfaces':
            result = GLib.Variant('(as)', (self.interfaces(path),))
        elif method == 'GetRoleName':
            role = 'push button' if path.endswith('/button') else 'entry' if path.endswith('/entry') else 'application'
            result = GLib.Variant('(s)', (role,))
        elif method == 'GetRole':
            result = GLib.Variant('(u)', (7,))
        elif method == 'GetState':
            result = GLib.Variant('(au)', ([0, 0],))
        elif method == 'GetActions':
            result = GLib.Variant('(a(sss))', ([('click', 'activate button', '')],))
        elif method == 'DoAction':
            self.activations += 1
            if self.hold_actions:
                self.pending_actions.append(invocation)
                return
            result = GLib.Variant('(b)', (True,))
        elif method == 'SetTextContents':
            self.text = args[0]
            result = GLib.Variant('(b)', (True,))
        elif method == 'GetExtents':
            result = GLib.Variant('((iiii))', ((10, 10, 100, 50),))
        elif method == 'ReadState':
            result = GLib.Variant('(s)', (json.dumps({'activations': self.activations, 'text': self.text}),))
        elif method == 'ReadProtocolState':
            result = GLib.Variant('(s)', (json.dumps({'accessibility_reads': self.accessibility_reads,
                                                    'pending_actions': len(self.pending_actions)}),))
        elif method == 'HoldActions':
            self.hold_actions = args[0]
            if not self.hold_actions:
                for pending in self.pending_actions:
                    pending.return_value(GLib.Variant('(b)', (True,)))
                self.pending_actions.clear()
            result = GLib.Variant('(b)', (True,))
        elif method == 'GetAddress':
            if self.hold_addresses:
                self.pending_addresses.append(invocation)
                return
            result = GLib.Variant('(s)', (self.address,))
        elif method == 'ListWindows':
            result = GLib.Variant('(s)', (json.dumps(self.windows),))
        elif method == 'ActivateWindow':
            for window in self.windows:
                window['focused'] = window['window_id'] == args[0]
            result = GLib.Variant('(bs)', (True, 'activated'))
        else:
            invocation.return_dbus_error('org.freedesktop.DBus.Error.UnknownMethod', method)
            return
        invocation.return_value(result)

    def property(self, connection, sender, path, interface, name):
        _, GLib = gio()
        if interface in (ACCESSIBLE, ACTION, EDITABLE, 'org.a11y.atspi.Component'):
            self.accessibility_reads += 1
        if name in ('IsEnabled', 'ScreenReaderEnabled'):
            return GLib.Variant('b', name == 'IsEnabled')
        if name == 'Name':
            suffix = ' button' if path.endswith('/button') else ' entry' if path.endswith('/entry') else ''
            return GLib.Variant('s', 'Fixture ' + str(self.label) + suffix)
        if name == 'Description':
            return GLib.Variant('s', '')
        if name == 'ChildCount':
            return GLib.Variant('i', len(self.children(path)))
        if name == 'NActions':
            return GLib.Variant('i', 1)
        return None


def application(address, label):
    _, GLib = gio()
    connection = connect(address)
    service = Service(connection, address, label)
    service.export(ROOT, [ACCESSIBLE, 'org.example.ScopeFixture'])
    service.export(ROOT + '/button', [ACCESSIBLE, ACTION, 'org.a11y.atspi.Component'])
    service.export(ROOT + '/entry', [ACCESSIBLE, EDITABLE, 'org.a11y.atspi.Component'])
    print(json.dumps({'pid': os.getpid(), 'name': connection.get_unique_name(), 'root': ROOT}), flush=True)
    GLib.MainLoop().run()


def rpc(proc, request):
    proc.stdin.write(json.dumps(request) + '\n')
    proc.stdin.flush()
    if 'id' not in request:
        return None
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if not select.select([proc.stdout], [], [], .2)[0]:
            continue
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError('MCP exited before its response')
        response = json.loads(line)
        if response.get('id') == request['id']:
            assert 'error' not in response, response
            result = response['result']
            assert not result.get('isError'), result
            if 'content' not in result:
                return result
            return result.get('structuredContent') or json.loads(next(
                c['text'] for c in result['content'] if c['type'] == 'text'))
    raise TimeoutError('MCP response exceeded fixture deadline')


def private_fixture(binary, case):
    Gio, GLib = gio()
    with tempfile.TemporaryDirectory(prefix='cul-scope-') as tmp:
        base = Path(tmp)
        for directory in ('runtime', 'config', 'bin'):
            (base / directory).mkdir(mode=0o700)
        # Diagnostic/input commands cannot reach any installed desktop process.
        for command in ('systemctl', 'gsettings', 'ydotool', 'ydotoold', 'xdotool',
                        'wtype', 'grim', 'gnome-screenshot', 'spectacle', 'wmctrl',
                        'xprop', 'hyprctl', 'niri', 'i3-msg', 'computer-use-linux-cosmic'):
            shim = base / 'bin' / command
            shim.write_text('#!/bin/sh\nexit 1\n')
            shim.chmod(0o700)
        (base / 'bin' / 'gnome-screenshot').write_text(
            '#!/bin/sh\ncase "$1" in -f) printf "capture\\n" >> "$CUL_CAPTURE_TRACE";; esac\nexit 1\n')
        if case == 'partial-coordinate-scroll':
            # This disposable backend records requested pointer commands; it
            # never contacts a daemon or an input device. Capability probes
            # have no runtime environment and therefore emit no trace.
            (base / 'bin' / 'ydotool').write_text('''#!/usr/bin/python3
import json
import os
import sys
args = sys.argv[1:]
if args and args[0] in ('help', '--help'):
    print('Usage: ydotool <cmd> <args>\\nAvailable commands:\\n  click\\n  mousemove\\n  type\\n  key\\n  debug')
elif 'XDG_RUNTIME_DIR' not in os.environ:
    sys.exit(0)
elif len(args) == 5 and args[0] == 'mousemove' and args[1] in ('--absolute', '--wheel') and args[2] == '--':
    with open(os.environ['CUL_POINTER_TRACE'], 'a') as trace:
        trace.write(json.dumps(args) + '\\n')
else:
    sys.exit(1)
''')
        config = base / 'bus.conf'
        deny_owner = ('<deny send_destination="org.freedesktop.DBus" '
                      'send_interface="org.freedesktop.DBus" '
                      'send_member="GetConnectionUnixProcessID"/>') if case == 'unknown-owner' else ''
        config_prefix = ('<busconfig><type>session</type><listen>unix:tmpdir=' +
                         str(base / 'runtime') + '</listen><policy context="default">'
                         '<allow own="*"/><allow send_destination="*"/>'
                         '<allow receive_sender="*"/>')
        config.write_text(config_prefix + deny_owner + '</policy></busconfig>')
        bus = subprocess.Popen(['/usr/bin/dbus-daemon', '--config-file=' + str(config), '--nofork',
                                '--nopidfile', '--print-address=1'], stdout=subprocess.PIPE, text=True)
        address = bus.stdout.readline().strip()
        assert address.startswith('unix:'), 'private bus did not start'
        env = {'PATH': str(base / 'bin') + ':/usr/bin:/bin', 'LANG': 'C.UTF-8',
               'HOME': str(base), 'XDG_CONFIG_HOME': str(base / 'config'),
               'XDG_RUNTIME_DIR': str(base / 'runtime'), 'DBUS_SESSION_BUS_ADDRESS': address,
               'DISPLAY': ':65535', 'WAYLAND_DISPLAY': 'fixture-no-display',
               'XAUTHORITY': str(base / 'no-xauthority'), 'HYPRLAND_INSTANCE_SIGNATURE': 'fixture',
               'DESKTOP_SESSION': 'fixture', 'XDG_SESSION_DESKTOP': 'fixture',
               'XDG_CURRENT_DESKTOP': 'fixture', 'XDG_SESSION_TYPE': 'wayland',
               'YDOTOOL_SOCKET': str(base / 'no-input.sock'), 'CU_DISABLE_ABS_POINTER': '1',
               'CUL_CAPTURE_TRACE': str(base / 'captures'),
               'CUL_POINTER_TRACE': str(base / 'pointer-commands'),
               'COMPUTER_USE_LINUX_SCREENSHOT_BACKEND': 'gnome-screenshot',
               'COMPUTER_USE_LINUX_FORCE_YDOTOOL_POINTER': '1',
               'COMPUTER_USE_LINUX_FORCE_YDOTOOL_KEYBOARD': '1'}
        processes = [bus]
        try:
            apps = []
            for label in ('A', 'B'):
                proc = subprocess.Popen(['/usr/bin/python3', __file__, '--application', address, label],
                                        env=env, stdout=subprocess.PIPE, text=True)
                processes.append(proc)
                apps.append(json.loads(proc.stdout.readline()))
            connection = connect(address)
            windows = [dict(window_id=i + 1, title='Fixture ' + label + ' window',
                            app_id='fixture-' + label.lower(), wm_class=None, pid=app['pid'],
                            bounds=dict(x=0, y=0, width=200, height=100), workspace=0,
                            focused=i == 0, hidden=False, client_type='wayland', backend='fixture')
                       for i, (label, app) in enumerate(zip(('A', 'B'), apps))]
            windows.append(dict(window_id=3, title=None, app_id=None, wm_class=None,
                                pid=None, bounds=dict(x=0, y=0, width=200, height=100),
                                workspace=0, focused=False, hidden=False,
                                client_type='wayland', backend='fixture'))
            service = Service(connection, address, roots=[(app['name'], app['root']) for app in apps], windows=windows)
            for name in ('org.a11y.Bus', 'org.a11y.atspi.Registry', WINDOW_SERVICE):
                own_name(connection, name)
            service.export('/org/a11y/bus', ['org.a11y.Bus', 'org.a11y.Status'])
            service.export(ROOT, [ACCESSIBLE])
            service.export(WINDOW_PATH, [WINDOW_SERVICE])
            loop = GLib.MainLoop()
            thread = threading.Thread(target=loop.run, daemon=True)
            thread.start()
            mcp = subprocess.Popen([binary, 'mcp'], env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, text=True)
            processes.append(mcp)
            rpc(mcp, {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
                'protocolVersion': '2024-11-05', 'capabilities': {},
                'clientInfo': {'name': 'accessibility-scope-regression', 'version': '1'}}})
            rpc(mcp, {'jsonrpc': '2.0', 'method': 'notifications/initialized'})
            counter = 1

            def call(name, arguments):
                nonlocal counter
                counter += 1
                return rpc(mcp, {'jsonrpc': '2.0', 'id': counter, 'method': 'tools/call',
                                 'params': {'name': name, 'arguments': arguments}})

            def read_app(app):
                result = connection.call_sync(app['name'], ROOT, 'org.example.ScopeFixture',
                    'ReadState', None, None, Gio.DBusCallFlags.NONE, 2000, None)
                return json.loads(result.unpack()[0])

            state = call('get_app_state', {'include_screenshot': False})
            names = {node.get('name') for node in state['accessibility_tree']}
            assert {'Fixture A button', 'Fixture B button'} <= names, state
            assert state['accessibility_error'] is None and not state['tree_scoped'], state
            if case == 'unresolved-title':
                state = call('get_app_state', {'title': 'Missing fixture title', 'include_screenshot': False})
                assert not state['accessibility_tree'], 'unresolved window title broadened the tree'
                assert state['accessibility_error'], state
            elif case == 'missing-pid':
                state = call('get_app_state', {'pid': apps[0]['pid'], 'include_screenshot': False})
                names = {node.get('name') for node in state['accessibility_tree']}
                assert 'Fixture A button' in names and 'Fixture B button' not in names, state
                assert state['tree_scoped'] and state['accessibility_error'] is None, state
                state = call('get_app_state', {'pid': 4000000000, 'include_screenshot': False})
                assert not state['accessibility_tree'] and state['accessibility_error'], state
                state = call('get_app_state', {'pid': 4000000000,
                             'app_name_or_bundle_identifier': 'Fixture B', 'include_screenshot': False})
                assert not state['accessibility_tree'], 'missing PID fell through to another application'
                assert state['accessibility_error'], state
                state = call('get_app_state', {'pid': apps[0]['pid'],
                             'app_name_or_bundle_identifier': 'Fixture B', 'include_screenshot': False})
                assert not state['accessibility_tree'] and state['accessibility_error'], state
            elif case == 'unknown-owner':
                button = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')
                entry = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A entry')
                before = read_app(apps[0])
                for action, arguments in [('perform_action', {'element_index': button['index']}),
                    ('set_value', {'element_index': entry['index'], 'value': 'replacement'}),
                    ('click', {'element_index': button['index']}),
                    ('scroll', {'element_index': button['index'], 'direction': 'down'})]:
                    result = call(action, arguments)
                    after = read_app(apps[0])
                    assert not result['ok'] and after == before, (action, result, before, after)
                    assert 'owner' in result['message'].lower(), (action, result)
            elif case == 'window-without-pid':
                button = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')
                before = read_app(apps[0])
                result = call('click', {'element_index': button['index'], 'window_id': 3})
                after = read_app(apps[0])
                assert not result['ok'] and after == before, ('window without PID permitted activation', before, after, result)
                assert 'process id' in result['message'].lower(), result
                result = call('scroll', {'element_index': button['index'], 'window_id': 3, 'direction': 'down'})
                assert not result['ok'] and 'process id' in result['message'].lower(), result
                state = call('get_app_state', {'window_id': 3, 'include_screenshot': False})
                assert not state['accessibility_tree'] and state['accessibility_error'], state
            elif case == 'scope-and-cache':
                nodes = state['accessibility_tree']
                button_a = next(n for n in nodes if n.get('name') == 'Fixture A button')
                button_b = next(n for n in nodes if n.get('name') == 'Fixture B button')
                entry_b = next(n for n in nodes if n.get('name') == 'Fixture B entry')
                result = call('perform_action', {'name': 'Fixture B button'})
                assert result['ok'] and read_app(apps[1])['activations'] == 1, result
                result = call('set_value', {'element_identifier': entry_b['object_ref'], 'value': 'unscoped'})
                assert result['ok'] and read_app(apps[1])['text'] == 'unscoped', result
                state = call('get_app_state', {'app_name_or_bundle_identifier': 'Fixture A', 'include_screenshot': False})
                assert state['tree_scoped'] and state['accessibility_error'] is None, state
                assert all('Fixture B' not in str(n.get('name')) for n in state['accessibility_tree']), state
                before_b = read_app(apps[1])
                for action, arguments in [('perform_action', {'element_identifier': button_b['object_ref']}),
                    ('set_value', {'element_identifier': entry_b['object_ref'], 'value': 'outside scope'})]:
                    result = call(action, arguments)
                    after_b = read_app(apps[1])
                    assert not result['ok'] and after_b == before_b, (action, result, before_b, after_b)
                button_a = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')
                entry_a = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A entry')
                result = call('set_value', {'element_index': entry_a['index'], 'value': 'scoped'})
                assert result['ok'] and read_app(apps[0])['text'] == 'scoped', result
                result = call('click', {'element_index': button_a['index']})
                assert result['ok'] and read_app(apps[0])['activations'] == 1, result
                before_a = read_app(apps[0])
                for action, arguments in [('click', {'element_index': button_a['index'], 'pid': apps[1]['pid']}),
                    ('scroll', {'element_index': button_a['index'], 'pid': apps[1]['pid'], 'direction': 'down'})]:
                    result = call(action, arguments)
                    assert not result['ok'] and 'target pid' in result['message'].lower(), result
                    assert read_app(apps[0]) == before_a, result
                state = call('get_app_state', {'title': 'Missing fixture title', 'include_screenshot': False})
                assert state['accessibility_error'] and not state['accessibility_tree'], state
                before_a = read_app(apps[0])
                for arguments in ({'element_index': button_a['index']}, {'name': 'Fixture A button'},
                                  {'element_identifier': button_a['object_ref']}):
                    result = call('perform_action', arguments)
                    after_a = read_app(apps[0])
                    assert not result['ok'] and after_a == before_a, (result, before_a, after_a)
                state = call('get_app_state', {'include_screenshot': False})
                result = call('perform_action', {'name': 'Fixture A button'})
                assert result['ok'] and read_app(apps[0])['activations'] == 2, result
            elif case == 'failed-scope-cache':
                button = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')
                before = read_app(apps[0])
                for target in ({'title': 'Missing fixture title'}, {'app_name_or_bundle_identifier': 'Missing fixture application'}):
                    state = call('get_app_state', {**target, 'include_screenshot': False})
                    result = call('perform_action', {'element_index': button['index']})
                    after = read_app(apps[0])
                    assert not result['ok'] and after == before, ('failed requested scope kept an actionable cached index', before, after, result)
                    result = call('perform_action', {'element_identifier': button['object_ref']})
                    after = read_app(apps[0])
                    assert not result['ok'] and after == before, ('failed requested scope permitted a direct identifier', before, after, result)
                    assert state['accessibility_error'] and not state['accessibility_tree'], state
            elif case == 'stale-owner':
                state = call('get_app_state', {'pid': apps[0]['pid'], 'include_screenshot': False})
                assert state['accessibility_error'] is None and state['tree_scoped'], state
                button = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')
                entry = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A entry')
                result = call('perform_action', {'element_index': button['index']})
                assert result['ok'] and read_app(apps[0])['activations'] == 1, result
                deny_owner = ('<deny send_destination="org.freedesktop.DBus" '
                              'send_interface="org.freedesktop.DBus" '
                              'send_member="GetConnectionUnixProcessID"/>')
                config.write_text(config_prefix + deny_owner + '</policy></busconfig>')
                connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                    'org.freedesktop.DBus', 'ReloadConfig', None, None, Gio.DBusCallFlags.NONE, 2000, None)
                before = read_app(apps[0])
                for action, arguments in [('perform_action', {'element_index': button['index']}),
                    ('set_value', {'element_index': entry['index'], 'value': 'stale owner'}),
                    ('click', {'element_index': button['index'], 'pid': apps[0]['pid']}),
                    ('scroll', {'element_index': button['index'], 'pid': apps[0]['pid'], 'direction': 'down'})]:
                    result = call(action, arguments)
                    after = read_app(apps[0])
                    assert not result['ok'] and after == before, (action, result, before, after)
                    assert 'owner' in result['message'].lower(), (action, result)
            elif case == 'owner-timeout':
                button = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')
                before = read_app(apps[0])
                service.hold_addresses = True
                started = time.monotonic()
                result = call('perform_action', {'element_index': button['index']})
                assert time.monotonic() - started < 3, 'owner lookup was not bounded'
                assert not result['ok'] and 'deadline' in result['message'].lower(), result
                assert read_app(apps[0]) == before, 'timed-out owner lookup activated button'
            elif case == 'partial-coordinate-scroll':
                button = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')
                pointer_trace = base / 'pointer-commands'
                failures = []

                def check_scroll(label, coordinates, extra, expected_error=None):
                    pointer_trace.unlink(missing_ok=True)
                    result = call('scroll', {'element_index': button['index'],
                                            'direction': 'down', **coordinates, **extra})
                    commands = [json.loads(line) for line in pointer_trace.read_text().splitlines()] if pointer_trace.exists() else []
                    print(json.dumps({'case': case, 'check': label, 'coordinates': coordinates,
                                      'result': result, 'pointer_commands': commands}), flush=True)
                    if expected_error:
                        if result['ok'] or expected_error not in result['message'].lower() or commands:
                            failures.append(label + ' did not refuse before pointer dispatch')
                    elif not result['ok'] or commands != [
                            ['mousemove', '--absolute', '--', '10', '20'],
                            ['mousemove', '--wheel', '--', '0', '-5']]:
                        failures.append(label + ' did not preserve explicit coordinates')

                # Missing either coordinate uses the element center, just as
                # omitting both does, so the explicit target PID must match.
                for label, coordinates in [('none', {}), ('x-only', {'x': 10}), ('y-only', {'y': 20})]:
                    check_scroll('pid-' + label, coordinates, {'pid': apps[1]['pid']}, 'target pid')
                check_scroll('pid-complete', {'x': 10, 'y': 20}, {'pid': apps[1]['pid']})

                deny_owner = ('<deny send_destination="org.freedesktop.DBus" '
                              'send_interface="org.freedesktop.DBus" '
                              'send_member="GetConnectionUnixProcessID"/>')
                config.write_text(config_prefix + deny_owner + '</policy></busconfig>')
                connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                    'org.freedesktop.DBus', 'ReloadConfig', None, None, Gio.DBusCallFlags.NONE, 2000, None)
                for label, coordinates in [('none', {}), ('x-only', {'x': 10}), ('y-only', {'y': 20})]:
                    check_scroll('owner-' + label, coordinates, {}, 'owner')
                check_scroll('owner-complete', {'x': 10, 'y': 20}, {})
                assert not failures, failures
            elif case == 'action-snapshot-serialization':
                state = call('get_app_state', {'pid': apps[0]['pid'], 'include_screenshot': False})
                assert state['tree_scoped'] and state['accessibility_error'] is None, state
                button = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture A button')

                def protocol_state(app):
                    result = connection.call_sync(app['name'], ROOT, 'org.example.ScopeFixture',
                        'ReadProtocolState', None, None, Gio.DBusCallFlags.NONE, 2000, None)
                    return json.loads(result.unpack()[0])

                def hold_actions(app, hold):
                    connection.call_sync(app['name'], ROOT, 'org.example.ScopeFixture',
                        'HoldActions', GLib.Variant('(b)', (hold,)), None,
                        Gio.DBusCallFlags.NONE, 2000, None)

                def start_call(name, arguments):
                    nonlocal counter
                    counter += 1
                    mcp.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': counter, 'method': 'tools/call',
                        'params': {'name': name, 'arguments': arguments}}) + '\n')
                    mcp.stdin.flush()
                    return counter

                responses = {}
                response_bytes = b''

                def poll_responses(wait):
                    nonlocal response_bytes
                    if not select.select([mcp.stdout], [], [], wait)[0]:
                        return
                    chunk = os.read(mcp.stdout.fileno(), 65536)
                    assert chunk, 'MCP exited during concurrent requests'
                    response_bytes += chunk
                    while b'\n' in response_bytes:
                        line, response_bytes = response_bytes.split(b'\n', 1)
                        response = json.loads(line)
                        if 'id' in response:
                            responses[response['id']] = response

                hold_actions(apps[0], True)
                action_id = start_call('perform_action', {'element_index': button['index']})
                deadline = time.monotonic() + 3
                while protocol_state(apps[0])['pending_actions'] != 1 and time.monotonic() < deadline:
                    poll_responses(.05)
                assert protocol_state(apps[0])['pending_actions'] == 1, 'application action did not reach the held backend reply'
                assert read_app(apps[0])['activations'] == 1, 'held action did not activate application A'
                previous_reads = protocol_state(apps[1])['accessibility_reads']
                snapshot_id = start_call('get_app_state', {'pid': apps[1]['pid'], 'include_screenshot': False})
                observed_reads = previous_reads
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline and snapshot_id not in responses:
                    poll_responses(.05)
                    observed_reads = protocol_state(apps[1])['accessibility_reads']
                snapshot_returned_early = snapshot_id in responses
                action_returned_early = action_id in responses
                pending_actions = protocol_state(apps[0])['pending_actions']
                print(json.dumps({'case': case, 'phase': 'held-action',
                    'snapshot_returned_before_release': snapshot_returned_early,
                    'action_returned_before_release': action_returned_early,
                    'application_b_read_before': previous_reads,
                    'application_b_read_after': observed_reads,
                    'pending_actions': pending_actions,
                    'application_a': read_app(apps[0]), 'application_b': read_app(apps[1])}), flush=True)
                hold_actions(apps[0], False)
                deadline = time.monotonic() + 10
                while not {action_id, snapshot_id} <= responses.keys() and time.monotonic() < deadline:
                    poll_responses(.1)
                assert {action_id, snapshot_id} <= responses.keys(), 'concurrent MCP requests did not finish after backend release'

                def tool_output(request_id):
                    response = responses[request_id]
                    assert 'error' not in response, response
                    result = response['result']
                    assert not result.get('isError'), result
                    return result.get('structuredContent') or json.loads(next(
                        item['text'] for item in result['content'] if item['type'] == 'text'))

                action = tool_output(action_id)
                state = tool_output(snapshot_id)
                assert action['ok'], action
                assert state['tree_scoped'] and state['accessibility_error'] is None, state
                assert all('Fixture A' not in str(n.get('name')) for n in state['accessibility_tree']), state
                button_b = next(n for n in state['accessibility_tree'] if n.get('name') == 'Fixture B button')
                result = call('perform_action', {'element_index': button_b['index']})
                assert result['ok'] and read_app(apps[1])['activations'] == 1, result
                assert read_app(apps[0])['activations'] == 1, 'the new snapshot index activated the earlier application'
                assert observed_reads > previous_reads, 'the concurrent snapshot did not reach application B'
                assert pending_actions == 1 and not action_returned_early, 'the delayed action reply was not still in flight'
                assert not snapshot_returned_early, 'snapshot replacement returned before the in-flight element action completed'
            elif case == 'screenshot-target':
                state = call('get_app_state', {'title': 'Missing fixture title', 'include_screenshot': True})
                assert state['screenshot'] is None and state['screenshot_error'], state
                assert not (base / 'captures').exists(), ('unresolved target started raw screenshot capture', (base / 'captures').read_text())
                assert 'could not be resolved' in state['screenshot_error'], state
            print(json.dumps({'case': case, 'passed': True}), flush=True)
            loop.quit()
        finally:
            for proc in reversed(processes):
                proc.terminate()
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', nargs='?')
    parser.add_argument('--case', default='all', choices=['all', 'unresolved-title', 'missing-pid',
        'unknown-owner', 'window-without-pid', 'scope-and-cache', 'failed-scope-cache', 'stale-owner',
        'owner-timeout', 'partial-coordinate-scroll', 'action-snapshot-serialization', 'screenshot-target'])
    parser.add_argument('--private', action='store_true')
    parser.add_argument('--application', nargs=2, metavar=('ADDRESS', 'LABEL'))
    args = parser.parse_args()
    if args.application:
        application(*args.application)
        return
    binary = str(Path(args.binary).resolve())
    if args.private:
        assert os.getpid() == 2 and os.getppid() == 1, 'private mode requires the bubblewrap PID namespace'
        private_fixture(binary, args.case)
    else:
        cases = ['unresolved-title', 'missing-pid', 'unknown-owner', 'window-without-pid',
                 'scope-and-cache', 'failed-scope-cache', 'stale-owner', 'owner-timeout',
                 'partial-coordinate-scroll', 'action-snapshot-serialization', 'screenshot-target'] if args.case == 'all' else [args.case]
        for case in cases:
            subprocess.run(['bwrap', '--ro-bind', '/', '/', '--dev', '/dev', '--proc', '/proc',
                            '--unshare-pid', '--unshare-net', '--unshare-ipc', '--die-with-parent',
                            '--tmpfs', '/tmp', '--tmpfs', '/run', '--ro-bind', binary, '/tmp/scope-mcp',
                            '--ro-bind', str(Path(__file__).resolve()), '/tmp/scope-fixture.py',
                            '--', '/usr/bin/python3', '/tmp/scope-fixture.py',
                            '/tmp/scope-mcp', '--private', '--case', case],
                           env={'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8'}, check=True, timeout=90)


if __name__ == '__main__':
    main()
