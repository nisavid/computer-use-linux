# Verify desktop input

Use an application readback to establish that text or an action landed. A
successful tool response establishes backend dispatch; a clipboard write,
portal reply, or kernel acknowledgement does not establish application input.

## Operate a target

1. Resolve the intended window with `list_windows` or `focused_window`, and
   obtain a fresh `get_app_state` for its PID or window selector. Use
   `include_screenshot: false` when the accessibility tree is sufficient.
   `app_name_or_bundle_identifier` limits the accessibility tree; a window
   selector supplies the screenshot target.
2. Require the requested scope to resolve. An empty tree or scope error is a
   reason to diagnose that target's accessibility support. Refresh the target
   before using its indices. Element identifiers and indices require a current
   owner matching the requested scope.
3. Use `type_text` for text and `press_key` for a deliberate key or chord.
   Keep terminal targets explicit: terminal paste bindings can differ from
   ordinary editable widgets in the same window.
4. Read the resulting value through AT-SPI or the application's own interface.
   Compare it with the requested text, including punctuation, Unicode, and
   newlines. A focused editable role alone does not verify insertion.
5. After an error or cancellation that may have submitted input, inspect the
   application before another mutating call. Cancellation before paste dispatch
   stops the new paste. Cleanup restores previous text only while the clipboard
   still contains the prepared value and preserves a detected external change;
   it does not restore MIME data or history. Dispatch already in progress
   finishes its cleanup. An ambiguous result does not authorize replay.

Readiness reports describe available capabilities. Layout qualification is
request-specific. Explicitly forced ydotool keyboard input retains its
layout-dependent compatibility behavior; use application readback before
depending on its text results.

Automatic raw typing requires a configured verified-daemon identity endpoint
and a supported native-Xorg profile for the captured request. A stock socket,
QWERTY label, or matching device name is insufficient. Unsupported modifiers,
repeat, controls, dead/Compose maps, focus, Wayland, and XWayland stop automatic
raw dispatch. Prefer an available literal backend; do not change the user's
keyboard state or force raw compatibility to turn a refusal into success.

## Reproduce a failure

Run backend regressions through the public MCP interface in disposable
fixtures. The repository's `scripts/accessibility_scope_test.py` and
`scripts/paste_backend_test.py` observe separate application counters, text,
clipboard state, and held-key cleanup. Their constructed services establish
protocol behavior, not a compositor or kernel-device qualification.

For compositor tests, use a hidden nested session with its own display, bus,
configuration, runtime directory, and fixture applications. Verify that every
service and window belongs to that session. Keep physical input backends
disabled and accept normal consent only inside the owned session. A Plasma
Activity or virtual desktop still shares the user's input seat.

For uinput or native Xorg device-binding tests, use a disposable guest with its
own kernel and input devices. Bind daemon, device, display, keymap, and modifier
state evidence to the tested source and binary. Two devices with the same name
are distinct test targets. Keep installed-host daemon or service changes under
their separate adoption authority.

Record the candidate revision, binary digest, requested text and target,
observed application value, backend events, cleanup result, and fixture type.
Use a pristine source build as the negative control. Re-run affected checks
when the candidate or its evidence dependencies change, and stop all owned
test processes when qualification is complete.
