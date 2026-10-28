const {Gio, GLib, GObject} = imports.gi;
const ByteArray = imports.byteArray;
const Meta = {WindowType: {DESKTOP: 9}, WindowClientType: {WAYLAND: 1, X11: 2}};
const Shell = {WindowTracker: {get_default: () => ({get_window_app: () => null})}};
const Clutter = {KEY_Escape: 0xff1b};
const Main = {overview: {visible: false}, activateWindow() {}, layoutManager: {monitors: [], primaryIndex: 0}};
let effects = 0;
const window = {get_id: () => 7, get_frame_rect: () => ({x: 0, y: 0, width: 10, height: 10}), move_frame() { effects++; }, move_resize_frame() { effects++; }};
Main.activateWindow = () => effects++;
const global = {get_window_actors: () => [{meta_window: window}], get_current_time: () => 0, display: {}};
const dialogs = [];
const ModalDialog = {ModalDialog: class {
    constructor() { this.buttons = []; this.contentLayout = {add_child() {}}; dialogs.push(this); }
    addButton(button) { this.buttons.push(button); }
    open() { return true; }
    close() { this.closed = true; }
}};
const Dialog = {MessageDialogContent: class {}};
const Extension = class {};
let [loaded, bytes] = GLib.file_get_contents((ARGV[0] || 'gnome-shell-extension/computer-use-linux@avifenesh.dev/extension.js'));
if (!loaded) throw new Error('cannot read extension');
let source = ByteArray.toString(bytes).replace(/^import .*;\n/gm, '').replace('export default class ', 'class ');
eval(source + '\nglobalThis.TestServer = WindowControlDBus;');
const SERVICE = 'dev.avifenesh.ComputerUseLinux.WindowControl';
const PATH = '/dev/avifenesh/ComputerUseLinux/WindowControl';
const server = new globalThis.TestServer();
const loop = new GLib.MainLoop(null, false);
let failure = null;
function check(value, description) { if (!value) throw new Error(description); print('PASS ' + description); }
function pause(ms) { return new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {resolve(); return GLib.SOURCE_REMOVE;})); }
async function until(predicate, description) {
    for (let i=0; i<200; i++) { if (predicate()) return; await pause(5); }
    throw new Error('Timed out: ' + description);
}
function call(client, method, signature='()', args=[], reply='(s)') {
    return new Promise((resolve, reject) => client.call(SERVICE, PATH, SERVICE, method,
        new GLib.Variant(signature, args), new GLib.VariantType(reply), Gio.DBusCallFlags.NONE, 1000, null,
        (connection, result) => { try {resolve(connection.call_finish(result).deepUnpack());} catch (error) {reject(error);} }));
}
function connect() { return Gio.DBusConnection.new_for_address_sync(GLib.getenv('DBUS_SESSION_BUS_ADDRESS'), Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, null, null); }
async function denied(client, method='ListWindows') {
    try {await call(client, method); throw new Error('unexpected operation success');}
    catch (error) {check(error.message.includes('AccessDenied'), method + ' returns real AccessDenied');}
}
async function run() {
    const first = connect();
    const second = connect();
    let ready = false;
    const readinessWatch = Gio.bus_watch_name_on_connection(first, SERVICE,
        Gio.BusNameWatcherFlags.NONE, () => { ready = true; }, null);
    try {
        await until(() => ready, 'extension owns its bus name');
    } finally {
        Gio.bus_unwatch_name(readinessWatch);
    }
    await denied(first);
    for (const [method, signature, args, reply] of [
        ['ActivateWindow', '(t)', [7], '(bs)'],
        ['MoveWindow', '(tii)', [7, 1, 2], '(bs)'],
        ['ResizeWindow', '(tii)', [7, 10, 20], '(bs)'],
        ['GetMonitorLayout', '()', [], '(s)'],
    ]) {
        let rejected = false;
        try { await call(first, method, signature, args, reply); }
        catch (error) { rejected = error.message.includes('AccessDenied'); }
        check(rejected, method + ' denies before operations');
    }
    const authorization = call(first, 'Authorize', '()', [], '(b)');
    const unix = GLib.getenv('DBUS_SESSION_BUS_ADDRESS').startsWith('unix:');
    if (unix) {
        await until(() => dialogs.length === 1, 'real bus PID lookup and dialog');
        dialogs.pop().buttons.find(button => button.label === 'Deny').action();
    }
    const [allowed] = await authorization;
    check(allowed === false, 'denial or unavailable identity never grants access');
    check(dialogs.length === 0, 'failed identity lookup never opens modal');
    const firstName = first.get_unique_name();
    check(server._clients.has(firstName), 'denial is retained for current unique name');
    first.close_sync(null);
    await until(() => !server._clients.has(firstName), 'real disconnect name watcher');
    check(!server._clients.has(firstName), 'real name disappearance removes denied client');
    if (!unix) server._identifyClient = client => server._showAuthorization(client, 1234);
    const pending = call(second, 'Authorize', '()', [], '(b)');
    await until(() => dialogs.length === 1, 'consent dialog stub');
    await denied(second);
    dialogs[0].buttons.find(button => button.label === 'Allow').action();
    const [ok] = await pending;
    check(ok === true, 'real Authorize invocation receives (b)');
    const [windows] = await call(second, 'ListWindows');
    check(JSON.parse(windows).length === 1, 'approved peer receives real ListWindows (s)');
    for (const [method, signature, args, reply] of [
        ['ActivateWindow', '(t)', [7], '(bs)'], ['MoveWindow', '(tii)', [7, 1, 2], '(bs)'],
        ['ResizeWindow', '(tii)', [7, 10, 20], '(bs)'], ['GetMonitorLayout', '()', [], '(s)'],
    ]) { const result = await call(second, method, signature, args, reply); check(method === 'GetMonitorLayout' || result[0] === true, method + ' approved control'); }
    check(effects === 3, 'all approved window operations retain side effects');
    // Denied peers must not permanently reserve approval slots.
    // Use actual private-bus connections; only this fixture's UI is stubbed.
    const rejectedPeers = [];
    for (let i = 0; i < 64; i++) {
        const peer = connect();
        rejectedPeers.push(peer);
        const request = call(peer, 'Authorize', '()', [], '(b)');
        await until(() => dialogs.length === 2, 'pressure-test denial dialog');
        dialogs.pop().buttons.find(button => button.label === 'Deny').action();
        check((await request)[0] === false, 'pressure-test peer remains denied');
    }
    const replacement = connect();
    let throttled = false;
    try { await call(replacement, 'Authorize', '()', [], '(b)'); }
    catch (error) { throttled = error.message.includes('AccessDenied'); }
    check(throttled, 'cache pressure throttles repeated prompt creation');
    await pause(5100);
    const replacementRequest = call(replacement, 'Authorize', '()', [], '(b)');
    await until(() => dialogs.length === 2, 'new sender can request consent at cache pressure');
    dialogs.pop().buttons.find(button => button.label === 'Deny').action();
    check((await replacementRequest)[0] === false, 'new sender denial is returned');
    check(!server._clients.has(rejectedPeers[0].get_unique_name()), 'oldest denial is evicted');
    await denied(rejectedPeers[0]);
    check(server._clients.get(second.get_unique_name())?.allowed === true,
        'denial pressure preserves an approved sender');
    check([...server._clients.values()].filter(client => !client.allowed).length === 64,
        'denial suppression stays bounded');
    await pause(5100);
    const recovered = call(rejectedPeers[0], 'Authorize', '()', [], '(b)');
    await until(() => dialogs.length === 2, 'evicted sender can request fresh approval');
    dialogs.pop().buttons.find(button => button.label === 'Allow').action();
    check((await recovered)[0] === true, 'explicit Allow succeeds after denial-cache pressure');
    check(JSON.parse((await call(rejectedPeers[0], 'ListWindows'))[0]).length === 1,
        'recovered sender receives protected data only after explicit Allow');
    replacement.close_sync(null);
    for (const peer of rejectedPeers)
        peer.close_sync(null);
    await until(() => server._clients.size === 1, 'pressure-test disconnect cleanup');

    const third = connect();
    await denied(third);
    let churnThrottled = false;
    try { await call(third, 'Authorize', '()', [], '(b)'); }
    catch (error) { churnThrottled = error.message.includes('AccessDenied'); }
    check(churnThrottled, 'disconnect churn cannot reset the prompt cooldown');
    await pause(5100);
    const secondName = second.get_unique_name();
    second.close_sync(null);
    await until(() => !server._clients.has(secondName), 'grant removal');
    check(!server._clients.has(secondName), 'real name disappearance removes approved client');
    const interrupted = call(third, 'Authorize', '()', [], '(b)');
    await until(() => dialogs.length === 2, 'pending authorization before destroy');
    const thirdName = third.get_unique_name();
    const dialog = dialogs[1];
    check(server._clients.get(thirdName)?.invocation !== null &&
        server._clients.get(thirdName)?.dialog === dialog,
        'destroy starts with a tracked pending request and open dialog');
    server.destroy();
    const [interruptedAllowed] = await interrupted;
    check(interruptedAllowed === false, 'destroy denies pending authorization');
    check(dialog.closed === true, 'destroy closes pending consent dialog');
    check(server._clients.size === 0, 'destroy releases retained clients');
    dialog.buttons.find(button => button.label === 'Allow').action();
    check(server._clients.size === 0, 'stale consent cannot restore a destroyed grant');
    third.close_sync(null);
}
run().catch(error => {failure = error; logError(error);}).finally(() => loop.quit());
GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 30, () => { failure = new Error('test deadline'); loop.quit(); return GLib.SOURCE_REMOVE; });
loop.run();
if (failure) imports.system.exit(1);
