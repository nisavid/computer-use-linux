import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import Clutter from 'gi://Clutter';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as Dialog from 'resource:///org/gnome/shell/ui/dialog.js';
import * as ModalDialog from 'resource:///org/gnome/shell/ui/modalDialog.js';

const SERVICE_NAME = 'dev.avifenesh.ComputerUseLinux.WindowControl';
const OBJECT_PATH = '/dev/avifenesh/ComputerUseLinux/WindowControl';
const BACKEND = 'gnome-shell-extension';

const WINDOW_CONTROL_XML = `
<node>
  <interface name="${SERVICE_NAME}">
    <method name="Authorize"><arg name="allowed" type="b" direction="out"/></method>
    <method name="ListWindows">
      <arg name="json" type="s" direction="out"/>
    </method>
    <method name="ActivateWindow">
      <arg name="window_id" type="t" direction="in"/>
      <arg name="ok" type="b" direction="out"/>
      <arg name="message" type="s" direction="out"/>
    </method>
    <method name="MoveWindow">
      <arg name="window_id" type="t" direction="in"/>
      <arg name="x" type="i" direction="in"/>
      <arg name="y" type="i" direction="in"/>
      <arg name="ok" type="b" direction="out"/>
      <arg name="message" type="s" direction="out"/>
    </method>
    <method name="ResizeWindow">
      <arg name="window_id" type="t" direction="in"/>
      <arg name="width" type="i" direction="in"/>
      <arg name="height" type="i" direction="in"/>
      <arg name="ok" type="b" direction="out"/>
      <arg name="message" type="s" direction="out"/>
    </method>
    <method name="GetMonitorLayout">
      <arg name="json" type="s" direction="out"/>
    </method>
  </interface>
</node>
`;

const WindowControlDBus = GObject.registerClass(
class WindowControlDBus extends GObject.Object {
    constructor() {
        super();
        this._clients = new Map();
        this._pendingAuthorization = null;
        this._lastPromptUsec = 0;
        this._promptRateLimited = false;
        this._destroyed = false;

        this._dbusObject = Gio.DBusExportedObject.wrapJSObject(
            WINDOW_CONTROL_XML, this);
        this._dbusObject.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.DBus.session.own_name(
            SERVICE_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            () => log(`Linux Computer Use Window Control lost DBus name ${SERVICE_NAME}`));
    }

    destroy() {
        this._destroyed = true;
        for (const client of this._clients.values())
            this._forgetClient(client);
        if (this._nameId) {
            Gio.DBus.session.unown_name(this._nameId);
            this._nameId = 0;
        }

        this._dbusObject?.unexport();
        this._dbusObject?.run_dispose();
        this._dbusObject = null;
    }

    ListWindowsAsync(_params, invocation) {
        if (!this._requireAuthorized(invocation))
            return;
        this._returnJson(invocation, this._listWindows());
    }

    ActivateWindowAsync([windowId], invocation) {
        if (!this._requireAuthorized(invocation))
            return;
        const requestedId = Number(windowId);
        const window = this._listMetaWindows().find(
            candidate => Number(candidate.get_id()) === requestedId);

        if (!window) {
            invocation.return_value(new GLib.Variant('(bs)', [
                false,
                `No window matched window_id ${requestedId}`,
            ]));
            return;
        }

        try {
            if (Main.overview.visible)
                Main.overview.hide();

            if (window.minimized && typeof window.unminimize === 'function')
                window.unminimize();

            Main.activateWindow(window, global.get_current_time());
            invocation.return_value(new GLib.Variant('(bs)', [
                true,
                `Activated window_id ${requestedId}`,
            ]));
        } catch (error) {
            invocation.return_value(new GLib.Variant('(bs)', [
                false,
                `Activation failed: ${error.message}`,
            ]));
        }
    }

    MoveWindowAsync([windowId, x, y], invocation) {
        this._withWindow(windowId, invocation, window => {
            // move_frame positions the frame rect (what list_windows reports).
            window.move_frame(true, x, y);
            return `Moved window_id ${Number(windowId)} to ${x},${y}`;
        });
    }

    ResizeWindowAsync([windowId, width, height], invocation) {
        this._withWindow(windowId, invocation, window => {
            if (width <= 0 || height <= 0)
                throw new Error(`invalid size ${width}x${height}`);
            // GNOME 49 removed Meta.Window.get_maximized() (use is_maximized())
            // and dropped the flags argument from unmaximize(). Support both
            // API generations: shell 45-48 (get_maximized + flags) and 49+.
            if (window.is_maximized?.())
                window.unmaximize();
            else if (window.get_maximized?.())
                window.unmaximize(Meta.MaximizeFlags.BOTH);
            const rect = window.get_frame_rect();
            window.move_resize_frame(true, rect.x, rect.y, width, height);
            return `Resized window_id ${Number(windowId)} to ${width}x${height}`;
        });
    }

    GetMonitorLayoutAsync(_params, invocation) {
        if (!this._requireAuthorized(invocation))
            return;
        const monitors = Main.layoutManager.monitors.map(monitor => ({
            index: monitor.index,
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
            primary: monitor.index === Main.layoutManager.primaryIndex,
            scale: monitor.geometry_scale ?? 1,
        }));
        this._returnJson(invocation, monitors);
    }

    _withWindow(windowId, invocation, action) {
        if (!this._requireAuthorized(invocation))
            return;
        const requestedId = Number(windowId);
        const window = this._listMetaWindows().find(
            candidate => Number(candidate.get_id()) === requestedId);

        if (!window) {
            invocation.return_value(new GLib.Variant('(bs)', [
                false,
                `No window matched window_id ${requestedId}`,
            ]));
            return;
        }

        try {
            const message = action(window);
            invocation.return_value(new GLib.Variant('(bs)', [
                true,
                message,
            ]));
        } catch (error) {
            invocation.return_value(new GLib.Variant('(bs)', [
                false,
                `Window operation failed: ${error.message}`,
            ]));
        }
    }

    _returnJson(invocation, value) {
        invocation.return_value(new GLib.Variant('(s)', [
            JSON.stringify(value),
        ]));
    }

    // Public bus names are not credentials. Grant only the bus-assigned sender.
    AuthorizeAsync(_params, invocation) {
        const sender = invocation.get_sender();
        if (this._destroyed || !sender?.startsWith(':')) {
            this._deny(invocation);
            return;
        }
        const existing = this._clients.get(sender);
        if (existing && !existing.invocation) {
            invocation.return_value(new GLib.Variant('(b)', [existing.allowed]));
            return;
        }
        const clients = [...this._clients.values()];
        const approved = clients.filter(client => client.allowed).length;
        const now = GLib.get_monotonic_time();
        // Denials suppress repeat prompts without owning approval capacity.
        // After cache pressure, keep a cooldown even if senders disconnect.
        if (this._pendingAuthorization || approved >= 64 ||
            (this._promptRateLimited && now - this._lastPromptUsec < 5_000_000)) {
            this._deny(invocation);
            return;
        }
        this._lastPromptUsec = now;
        const client = {sender, invocation, allowed: false, watchId: 0,
            timeoutId: 0, dialog: null, dialogOpened: false,
            cancellable: new Gio.Cancellable()};
        this._clients.set(sender, client);
        this._pendingAuthorization = client;
        client.timeoutId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 20, () => {
            client.timeoutId = 0;
            this._finishAuthorization(client, false);
            return GLib.SOURCE_REMOVE;
        });
        client.watchId = Gio.bus_watch_name_on_connection(
            Gio.DBus.session, sender, Gio.BusNameWatcherFlags.NONE,
            () => this._identifyClient(client), () => this._forgetClient(client));
    }

    _identifyClient(client) {
        if (this._pendingAuthorization !== client)
            return;
        Gio.DBus.session.call('org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', 'GetConnectionUnixProcessID',
            new GLib.Variant('(s)', [client.sender]), new GLib.VariantType('(u)'),
            Gio.DBusCallFlags.NONE, 1000, client.cancellable, (connection, result) => {
                if (this._pendingAuthorization !== client)
                    return;
                try {
                    const [pid] = connection.call_finish(result).deepUnpack();
                    if (!Number.isInteger(pid) || pid <= 0)
                        throw new Error('Invalid process ID');
                    this._showAuthorization(client, pid);
                } catch (_error) {
                    this._finishAuthorization(client, false);
                }
            });
    }

    _showAuthorization(client, pid) {
        const dialog = new ModalDialog.ModalDialog();
        client.dialog = dialog;
        dialog.contentLayout.add_child(new Dialog.MessageDialogContent({
            title: 'Allow desktop window control?',
            description: `Local process ${pid} (connection ${client.sender}) requests window titles, monitor layout, focus, movement, and resizing. Allow only if you just requested desktop automation. Approval lasts until this connection closes or the extension reloads.`,
        }));
        dialog.addButton({label: 'Deny', key: Clutter.KEY_Escape, default: true,
            action: () => this._finishAuthorization(client, false)});
        dialog.addButton({label: 'Allow',
            action: () => this._finishAuthorization(client, true)});
        client.dialogOpened = dialog.open();
        if (!client.dialogOpened)
            this._finishAuthorization(client, false);
    }

    _finishAuthorization(client, allowed) {
        if (this._pendingAuthorization !== client)
            return;
        this._pendingAuthorization = null;
        client.allowed = allowed && !this._destroyed && this._clients.get(client.sender) === client;
        if (client.timeoutId)
            GLib.source_remove(client.timeoutId);
        client.timeoutId = 0;
        client.cancellable.cancel();
        if (client.dialogOpened)
            client.dialog.close();
        else
            client.dialog?.destroy();
        client.dialog = null;
        const invocation = client.invocation;
        client.invocation = null;
        invocation.return_value(new GLib.Variant('(b)', [client.allowed]));
        // Keep at most 64 denied senders. Eviction never creates a grant;
        // it only permits a later request to ask for consent again.
        const denied = [...this._clients.values()].filter(
            candidate => !candidate.allowed && !candidate.invocation);
        if (denied.length >= 64)
            this._promptRateLimited = true;
        for (const expired of denied.slice(0, Math.max(0, denied.length - 64)))
            this._forgetClient(expired);
    }

    _forgetClient(client) {
        this._finishAuthorization(client, false);
        client.allowed = false;
        this._clients.delete(client.sender);
        if (client.watchId)
            Gio.bus_unwatch_name(client.watchId);
        client.watchId = 0;
    }

    _requireAuthorized(invocation) {
        if (!this._destroyed && this._clients.get(invocation.get_sender())?.allowed)
            return true;
        this._deny(invocation);
        return false;
    }

    _deny(invocation) {
        invocation.return_dbus_error('org.freedesktop.DBus.Error.AccessDenied',
            'Desktop window control requires approval through Authorize on this connection');
    }

    _listWindows() {
        return this._listMetaWindows()
            .map(window => this._windowInfo(window))
            .filter(window => window !== null);
    }

    _listMetaWindows() {
        return global.get_window_actors()
            .map(actor => actor.meta_window)
            .filter(window => window && !window.is_override_redirect?.())
            .filter(window => window.get_window_type?.() !== Meta.WindowType.DESKTOP);
    }

    _windowInfo(window) {
        if (!window)
            return null;

        const app = Shell.WindowTracker.get_default().get_window_app(window);
        const rect = window.get_frame_rect();
        const workspace = window.get_workspace?.();

        return {
            window_id: Number(window.get_id()),
            title: window.get_title?.() ?? null,
            app_id: app?.get_id?.() ?? null,
            wm_class: window.get_wm_class?.() ?? null,
            pid: window.get_pid?.() ?? null,
            bounds: rect ? {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            } : null,
            workspace: workspace?.index?.() ?? null,
            focused: global.display.focus_window === window && !Main.overview.visible,
            hidden: window.minimized ?? false,
            client_type: clientTypeName(window.get_client_type?.()),
            backend: BACKEND,
        };
    }
});

function clientTypeName(value) {
    if (value === undefined || value === null)
        return null;
    if (value === Meta.WindowClientType.WAYLAND)
        return 'wayland';
    if (value === Meta.WindowClientType.X11)
        return 'x11';
    return 'unknown';
}

export default class ComputerUseLinuxWindowControlExtension extends Extension {
    enable() {
        this._dbusServer = new WindowControlDBus();
    }

    disable() {
        this._dbusServer?.destroy();
        this._dbusServer = null;
    }
}
