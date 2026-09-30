#!/usr/bin/env python3
"""Opt-in public MCP/readback test inside an explicitly provisioned guest.

Requires native Xorg, an owned verified ydotool daemon/device, PyQt6, and
window metadata/focus support. The caller prepares the keyboard profile and
chooses a positive or pre-dispatch refusal case. No host setup is performed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import shutil
import socket
import struct
import subprocess
import tempfile
import time


APP = r'''
import ctypes, json, os, struct, time
from pathlib import Path
from PyQt6.QtCore import QAbstractNativeEventFilter, QTimer
from PyQt6.QtWidgets import QApplication, QWidget, QVBoxLayout, QLineEdit
root = Path(os.environ['CUL_NATIVE_FIXTURE_ROOT'])
held = set()
class Entry(QLineEdit):
    def keyPressEvent(self, event):
        held.add(event.nativeScanCode())
        with (root / 'events.jsonl').open('a') as stream:
            stream.write(json.dumps({'event': 'press', 'symbol': event.nativeVirtualKey(),
                'scan': event.nativeScanCode(), 'text': event.text(),
                'modifiers': event.modifiers().value,
                'time': time.monotonic()}) + '\n')
        super().keyPressEvent(event)
        (root / 'readback.txt').write_text(self.text())
    def keyReleaseEvent(self, event):
        held.discard(event.nativeScanCode())
        with (root / 'events.jsonl').open('a') as stream:
            stream.write(json.dumps({'event': 'release', 'scan': event.nativeScanCode(),
                                    'time': time.monotonic()}) + '\n')
        super().keyReleaseEvent(event)
app = QApplication([])
app.setApplicationName('cul-native-input-fixture')
window = QWidget()
window.setWindowTitle('CUL native input fixture')
layout = QVBoxLayout(window)
entry = Entry()
entry.setText('LEFT_OLD_RIGHT')
entry.setSelection(5, 3)
layout.addWidget(entry)
window.resize(480, 100)
window.show()
entry.setFocus()
(root / 'readback.txt').write_text(entry.text())
(root / 'events.jsonl').write_text('')
(root / 'identity.json').write_text(json.dumps({
    'pid': os.getpid(), 'xid': int(window.winId())}))
from Xlib import display
connection = display.Display()
readback_atom = connection.intern_atom('_CUL_NATIVE_READBACK')
connection.close()
class ReadbackBarrier(QAbstractNativeEventFilter):
    def nativeEventFilter(self, kind, message):
        if bytes(kind) == b'xcb_generic_event_t' and message is not None:
            packet = ctypes.string_at(int(message), 32)
            if packet[0] & 0x7f == 33:
                xid, atom, nonce = struct.unpack_from('=III', packet, 4)
                if xid == int(window.winId()) and atom == readback_atom:
                    def reply():
                        path = root / ('barrier-' + str(nonce) + '.json')
                        pending = path.with_suffix('.tmp')
                        pending.write_text(json.dumps({
                            'nonce': nonce, 'value': entry.text(), 'held_scans': sorted(held),
                            'events': [json.loads(line) for line in
                                       (root / 'events.jsonl').read_text().splitlines()]}))
                        pending.replace(path)
                    QTimer.singleShot(0, reply)
                    return True, 0
        return False, 0
barrier = ReadbackBarrier()
app.installNativeEventFilter(barrier)
QTimer.singleShot(250, lambda: print('CUL_NATIVE_READY', flush=True))
app.exec()
'''


class Client:
    def __init__(self, process):
        self.process = process
        self.next_id = 0

    def rpc(self, method, params=None, notification=False):
        self.next_id += 1
        request = {'jsonrpc': '2.0', 'method': method}
        if not notification:
            request['id'] = self.next_id
        if params is not None:
            request['params'] = params
        self.process.stdin.write(json.dumps(request) + '\n')
        self.process.stdin.flush()
        if notification:
            return None
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            if not select.select([self.process.stdout], [], [], 0.1)[0]:
                continue
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError('MCP exited before response')
            response = json.loads(line)
            if response.get('id') != request['id']:
                continue
            if 'error' in response:
                raise RuntimeError(response['error'])
            return response['result']
        raise TimeoutError(method)

    def tool(self, name, arguments):
        response = self.rpc('tools/call', {'name': name, 'arguments': arguments})
        if response.get('structuredContent') is not None:
            return response['structuredContent']
        return json.loads(next(item['text'] for item in response['content']
                               if item['type'] == 'text'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--ydotool', type=Path, required=True)
    parser.add_argument('--identity-socket', type=Path, required=True)
    parser.add_argument('--raw-socket', type=Path, required=True)
    parser.add_argument('--display', required=True)
    parser.add_argument('--guest-instance', required=True)
    parser.add_argument('--text', required=True)
    parser.add_argument('--expect', choices=('inserted', 'refused'), required=True)
    parser.add_argument('--save-evidence', type=Path, required=True)
    args = parser.parse_args()
    if os.environ.get('COMPUTER_USE_TEST_NATIVE_GUEST') != '1':
        parser.error('set COMPUTER_USE_TEST_NATIVE_GUEST=1 in the disposable guest')
    if Path('/var/lib/cloud/data/instance-id').read_text().strip() != args.guest_instance:
        parser.error('the provisioned guest instance identity does not match')
    match = re.fullmatch(r':([0-9]+)(?:\.([0-9]+))?', args.display)
    if match is None or int(match[1]) > 65535:
        parser.error('an explicit local Unix display is required before GUI startup')
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(2)
        connection.connect('/tmp/.X11-unix/X' + str(int(match[1])))
        peer, _, _ = struct.unpack('3i', connection.getsockopt(socket.SOL_SOCKET,
                                                             socket.SO_PEERCRED, 12))
        assert peer > 0 and Path('/proc/' + str(peer) + '/exe').resolve().name == 'Xorg'
        assert os.stat('/proc/self/ns/pid').st_ino == os.stat('/proc/' + str(peer) + '/ns/pid').st_ino
    root = Path(tempfile.mkdtemp(prefix='cul-native-mcp-'))
    root.chmod(0o700)
    env = os.environ.copy()
    env.pop('WAYLAND_DISPLAY', None)
    env['XDG_SESSION_TYPE'] = 'x11'
    env['DISPLAY'] = args.display
    env['QT_QPA_PLATFORM'] = 'xcb'
    env['CUL_NATIVE_FIXTURE_ROOT'] = str(root)
    env['PATH'] = str(args.ydotool.resolve().parent) + os.pathsep + env['PATH']
    assert Path(shutil.which('ydotool', path=env['PATH'])).resolve() == args.ydotool.resolve()
    env['YDOTOOL_SOCKET'] = str(args.raw_socket)
    env['COMPUTER_USE_LINUX_YDOTOOL_IDENTITY_SOCKET'] = str(args.identity_socket)
    env['CU_DISABLE_ABS_POINTER'] = '1'
    for name in ('COMPUTER_USE_LINUX_FORCE_YDOTOOL_KEYBOARD',
                 'COMPUTER_USE_LINUX_FORCE_XDOTOOL_KEYBOARD',
                 'COMPUTER_USE_LINUX_FORCE_PORTAL_KEYBOARD'):
        env.pop(name, None)
    app = mcp = None
    report = {'fixture': 'owned native Xorg/uinput guest',
              'guest_instance': args.guest_instance,
              'fixture_root': str(root), 'requested': args.text,
              'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest()}
    try:
        with (root / 'app-stderr.log').open('w') as app_err, \
                (root / 'mcp-stderr.log').open('w') as mcp_err:
            app = subprocess.Popen(['/usr/bin/python3', '-X', 'faulthandler', '-c', APP], env=env,
                                   stdout=subprocess.PIPE, stderr=app_err, bufsize=0)
            deadline = time.monotonic() + 10
            while True:
                assert time.monotonic() < deadline, 'fixture not ready'
                if not select.select([app.stdout], [], [], 0.1)[0]:
                    continue
                line = app.stdout.readline().decode()
                assert line, 'fixture exited before readiness: ' + str(app.poll())
                if line.strip() == 'CUL_NATIVE_READY':
                    break
                with (root / 'app-stdout.log').open('a') as output:
                    output.write(line)
            identity = json.loads((root / 'identity.json').read_text())
            assert identity['pid'] == app.pid
            mcp = subprocess.Popen([str(args.binary.resolve()), 'mcp'], env=env,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=mcp_err, text=True)
            client = Client(mcp)
            client.rpc('initialize', {'protocolVersion': '2024-11-05',
                       'capabilities': {}, 'clientInfo': {
                           'name': 'native-guest-readback', 'version': '1'}})
            client.rpc('notifications/initialized', notification=True)
            windows = client.tool('list_windows', {})
            candidates = [window for window in windows['windows']
                          if window.get('pid') == app.pid
                          and window.get('window_id') == identity['xid']
                          and window.get('title') == 'CUL native input fixture']
            assert len(candidates) == 1, 'owned target not independently resolved'
            target = candidates[0]
            assert client.tool('activate_window', {'window_id': identity['xid']})['ok']
            result = client.tool('type_text', {'window_id': identity['xid'],
                                 'pid': app.pid, 'text': args.text})
            from Xlib import display, protocol
            connection = display.Display(args.display)
            nonce = int.from_bytes(os.urandom(4), 'little')
            atom = connection.intern_atom('_CUL_NATIVE_READBACK')
            connection.create_resource_object('window', identity['xid']).send_event(
                protocol.event.ClientMessage(window=identity['xid'], client_type=atom,
                                             data=(32, [nonce, 0, 0, 0, 0])), event_mask=0)
            connection.sync()
            connection.close()
            barrier_path = root / ('barrier-' + str(nonce) + '.json')
            deadline = time.monotonic() + 5
            while not barrier_path.exists():
                if time.monotonic() >= deadline:
                    raise TimeoutError('owned application readback barrier')
                time.sleep(0.01)
            snapshot = json.loads(barrier_path.read_text())
            assert snapshot['nonce'] == nonce
            observed, events = snapshot['value'], snapshot['events']
            report.update(target=target, tool=result, observed=observed, events=events,
                          application_barrier=snapshot)
            if args.expect == 'inserted':
                assert result['ok'], result
                assert 'Verified raw keyboard strokes' in result['message'], result
                assert observed == 'LEFT_' + args.text + '_RIGHT', report
                assert not snapshot['held_scans'], report
            else:
                assert not result['ok'], result
                assert observed == 'LEFT_OLD_RIGHT' and not events, report
            report['pass'] = True
    except BaseException as error:
        report['pass'] = False
        report['failure'] = str(error)
        raise
    finally:
        for process in (mcp, app):
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        args.save_evidence.write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report))


if __name__ == '__main__':
    main()
