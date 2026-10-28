#!/usr/bin/python3
"""Real GNOME Shell 46/X11 consent smoke. Requires an isolated Xvfb session.

The extension is not transformed or monkey-patched. Screenshots and real XTest
keyboard/mouse input drive its actual GNOME modal. No Shell Eval API is used.
"""
import concurrent.futures
import csv
import io
import json
import os
from pathlib import Path
import subprocess
import time

from PIL import Image

import gi
gi.require_version('Gio', '2.0')
from gi.repository import Gio, GLib

SERVICE = 'dev.avifenesh.ComputerUseLinux.WindowControl'
PATH = '/dev/avifenesh/ComputerUseLinux/WindowControl'
UUID = 'computer-use-linux@avifenesh.dev'
ARTIFACTS = Path(os.environ['GNOME_LIVE_ARTIFACTS'])
TITLE = 'CUL live consent test window'
POOL = concurrent.futures.ThreadPoolExecutor(max_workers=2)
TEST_WINDOW = None
METHODS = [
    ('ListWindows', '()', (), '(s)'),
    ('ActivateWindow', '(t)', (1,), '(bs)'),
    ('MoveWindow', '(tii)', (1, 180, 160), '(bs)'),
    ('ResizeWindow', '(tii)', (1, 420, 280), '(bs)'),
    ('GetMonitorLayout', '()', (), '(s)'),
]


def check(condition, description):
    if not condition:
        raise AssertionError(description)
    print('PASS ' + description, flush=True)


def until(predicate, description, timeout=30):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = predicate()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError('Timed out: ' + description)


def connect():
    return Gio.DBusConnection.new_for_address_sync(
        os.environ['DBUS_SESSION_BUS_ADDRESS'],
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
        Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)


def call(connection, method, signature='()', args=(), reply='(s)', timeout=5000):
    return connection.call_sync(SERVICE, PATH, SERVICE, method,
        GLib.Variant(signature, args), GLib.VariantType.new(reply),
        Gio.DBusCallFlags.NONE, timeout, None).unpack()


def has_owner(connection):
    return connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
        'org.freedesktop.DBus', 'NameHasOwner', GLib.Variant('(s)', (SERVICE,)),
        GLib.VariantType.new('(b)'), Gio.DBusCallFlags.NONE, 2000, None).unpack()[0]


def denied(connection):
    for method, signature, args, reply in METHODS:
        try:
            call(connection, method, signature, args, reply)
        except GLib.Error as error:
            check(Gio.DBusError.get_remote_error(error) ==
                  'org.freedesktop.DBus.Error.AccessDenied',
                  method + ' rejects the unapproved unique sender')
        else:
            raise AssertionError(method + ' unexpectedly succeeded')


def screenshot(name):
    path = ARTIFACTS / (name + '.png')
    subprocess.run(['scrot', '--overwrite', str(path)], check=True)
    return path


def buttons(name):
    """Locate both real rendered button labels, never assume click coordinates."""
    path = screenshot(name)
    # Shell's white-on-gray modal is otherwise classified as an image by OCR.
    # Preserve the screenshot; isolate its bright text without moving pixels.
    ocr_path = ARTIFACTS / (name + '-ocr.png')
    with Image.open(path) as captured:
        captured.convert('L').point(lambda value: 0 if value > 180 else 255).save(ocr_path)
    result = subprocess.run(['tesseract', str(ocr_path), 'stdout', '--psm', '6', 'tsv'],
                            check=True, capture_output=True, text=True)
    (ARTIFACTS / (name + '.tsv')).write_text(result.stdout)
    words = list(csv.DictReader(io.StringIO(result.stdout), delimiter='\t'))
    for deny in words:
        if deny.get('text', '').strip() != 'Deny':
            continue
        for allow in words:
            if allow.get('text', '').strip() != 'Allow':
                continue
            dy, ay = int(deny['top']), int(allow['top'])
            dx, ax = int(deny['left']), int(allow['left'])
            if abs(dy - ay) < 25 and ax > dx:
                return {w['text'].strip(): (
                    int(w['left']) + int(w['width']) // 2,
                    int(w['top']) + int(w['height']) // 2,
                ) for w in (deny, allow)}
    return None


def authorize(connection, phase, choice):
    future = POOL.submit(call, connection, 'Authorize', '()', (), '(b)', 35000)
    locations = until(lambda: buttons(phase), 'real rendered Allow/Deny modal', 10)
    check(not future.done(), phase + ': authorization remains pending at visible modal')
    if choice in ('Allow', 'Deny'):
        x, y = locations[choice]
        subprocess.run(['xdotool', 'mousemove', '--sync', str(x), str(y),
                        'click', '1'], check=True)
    elif choice == 'Escape':
        subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Escape'], check=True)
    elif choice == 'Return':
        subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Return'], check=True)
    elif choice != 'Timeout':
        raise ValueError(choice)
    allowed = future.result(timeout=35)[0]
    check(allowed == (choice == 'Allow'), phase + ': actual modal choice returned expected grant')
    until(lambda: not buttons(phase + '-after'), 'actual consent modal closes', 10)
    return allowed


def windows(connection):
    return json.loads(call(connection, 'ListWindows')[0])


def target(connection):
    return next((w for w in windows(connection) if w['title'] == TITLE), None)


def controls(connection):
    window = until(lambda: target(connection), 'test xterm visible to actual Meta.Window')
    wid = window['window_id']
    check(call(connection, 'ActivateWindow', '(t)', (wid,), '(bs)')[0],
          'approved ActivateWindow succeeds')
    until(lambda: target(connection)['focused'], 'actual test window receives focus')
    check(call(connection, 'MoveWindow', '(tii)', (wid, 180, 160), '(bs)')[0],
          'approved MoveWindow succeeds')
    until(lambda: abs(target(connection)['bounds']['x'] - 180) < 10 and
                  abs(target(connection)['bounds']['y'] - 160) < 10,
          'real window moved to requested frame coordinates')
    before = target(connection)['bounds']
    width = 620 if before['width'] < 500 else 420
    check(call(connection, 'ResizeWindow', '(tii)', (wid, width, 280), '(bs)')[0],
          'approved ResizeWindow succeeds')
    until(lambda: abs(target(connection)['bounds']['width'] - before['width']) > 50,
          'actual test window size changes')
    monitors = json.loads(call(connection, 'GetMonitorLayout')[0])
    check(bool(monitors) and monitors[0]['width'] > 0,
          'approved GetMonitorLayout returns the real Xvfb monitor')
    screenshot('approved-controls')


def main():
    global TEST_WINDOW
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    observer = connect()
    until(lambda: has_owner(observer), 'unmodified extension exported in real Shell', 60)
    print('GNOME extension owner is ready', flush=True)
    # Do not let a long-lived window retain the assertions pipe to tee.
    TEST_WINDOW = subprocess.Popen(
        ['xterm', '-T', TITLE, '-geometry', '80x24+40+80'],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # Close the initial overview if the distro starts in overview mode.
    subprocess.run(['xdotool', 'key', '--clearmodifiers', 'Escape'], check=True)
    client = connect()
    denied(client)
    client.close_sync(None)

    for choice in ('Deny', 'Escape', 'Return', 'Timeout'):
        client = connect()
        started = time.monotonic()
        authorize(client, choice.lower(), choice)
        if choice == 'Timeout':
            check(time.monotonic() - started >= 18,
                  'timeout exercised actual 20-second consent deadline')
        denied(client)
        check(call(client, 'Authorize', '()', (), '(b)')[0] is False,
              'denial remains cached on the same live connection')
        client.close_sync(None)

    abandoned = connect()
    abandoned_request = POOL.submit(call, abandoned, 'Authorize', '()', (), '(b)', 35000)
    until(lambda: buttons('disconnect-pending'), 'pending modal before disconnect', 10)
    check(not abandoned_request.done(), 'disconnected request was pending at actual modal')
    abandoned.close_sync(None)
    until(lambda: not buttons('disconnect-pending-after'),
          'actual modal closes when pending sender disconnects', 10)
    try:
        result = abandoned_request.result(timeout=5)
    except GLib.Error:
        pass  # The local transport closed while its synchronous call was pending.
    else:
        check(result == (False,), 'disconnect never resolves authorization as allowed')

    approved = connect()
    authorize(approved, 'allow', 'Allow')
    controls(approved)
    peer = connect()
    denied(peer)
    peer.close_sync(None)
    old_name = approved.get_unique_name()
    approved.close_sync(None)
    fresh = connect()
    check(fresh.get_unique_name() != old_name, 'disconnect gives a fresh unique name')
    denied(fresh)
    authorize(fresh, 'allow-after-disconnect', 'Allow')

    subprocess.run(['gnome-extensions', 'disable', UUID], check=True)
    until(lambda: not has_owner(observer), 'extension releases service on disable')
    subprocess.run(['gnome-extensions', 'enable', UUID], check=True)
    until(lambda: has_owner(observer), 'extension exports fresh service on enable')
    denied(fresh)
    authorize(fresh, 'allow-after-extension-reload', 'Allow')
    controls(fresh)
    fresh.close_sync(None)
    observer.close_sync(None)
    print('PASS live GNOME Shell modal and compositor smoke completed', flush=True)


if __name__ == '__main__':
    try:
        main()
    except BaseException:
        screenshot('failure')
        raise
    finally:
        if TEST_WINDOW is not None:
            TEST_WINDOW.terminate()
            try:
                TEST_WINDOW.wait(timeout=3)
            except subprocess.TimeoutExpired:
                TEST_WINDOW.kill()
                TEST_WINDOW.wait()
        POOL.shutdown(wait=False, cancel_futures=True)
