# Check cancellation during KDE paste

This fixture drives the built `computer-use-linux mcp` executable over stdio
against private fake RemoteDesktop and Klipper services. It first establishes
one uncancelled physical chord and clipboard restoration. It then runs the
selected cancellation case with fresh fake services. The services record
keyboard notifications, clipboard changes, and effects on a synthetic field.

Set `KDE_PASTE_CASE` to select one case per invocation. Each invocation
creates a fresh evidence directory.

- `cancel-prepared` is the default. Fake Klipper accepts the text and holds
  the write reply.
- `cancel-start-success` holds the portal `Start` response and releases a
  keyboard grant after cancellation.
- `cancel-start-denied` releases a denial after cancellation. A subsequent
  Escape can obtain a new portal session.
- `cancel-dispatched` accepts the first Ctrl keycode press and holds its
  reply. The client queues Escape while that reply remains held.

Use a fresh private evidence directory. Set `CANDIDATE_BINARY` to the
absolute path of the admitted build to check. The command below assumes a
merged `/usr` Linux layout with Python D-Bus, PyGObject, GLib, `dbus-daemon`,
and Bubblewrap. The namespace operation must succeed before the server
launches.

```sh
binary=$(realpath "$CANDIDATE_BINARY")
fixture=$(realpath scripts/kde_paste_cancellation_test.py)
evidence=$(mktemp -d "${TMPDIR:-/tmp}/cul-paste-evidence.XXXXXXXX")
fixture_case=${KDE_PASTE_CASE:-cancel-prepared}

printf 'root:x:0:0:fixture:/tmp:/bin/sh\n' > "$evidence/passwd"
printf 'root:x:0:\n' > "$evidence/group"
git rev-parse HEAD > "$evidence/source-commit"
printf '%s\n' "$fixture_case" > "$evidence/scenario"
sha256sum "$binary" "$fixture" docs/testing/kde-paste-cancellation.md \
  > "$evidence/SHA256SUMS"

timeout --kill-after=3s 50s /usr/bin/bwrap \
  --unshare-user --uid 0 --gid 0 --unshare-pid --unshare-net --unshare-ipc \
  --die-with-parent --new-session --clearenv \
  --ro-bind /usr /usr --symlink usr/bin /bin \
  --symlink usr/lib /lib --symlink usr/lib /lib64 \
  --proc /proc --dev /dev --tmpfs /tmp --dir /run --dir /etc \
  --ro-bind "$evidence/passwd" /etc/passwd \
  --ro-bind "$evidence/group" /etc/group \
  --ro-bind "$binary" /cul --ro-bind "$fixture" /fixture.py \
  --bind "$evidence" /evidence --chdir /tmp \
  /usr/bin/python3 -I -B /fixture.py /cul /evidence \
  "$(readlink /proc/self/ns/mnt)" "$(readlink /proc/self/ns/pid)" \
  "$(readlink /proc/self/ns/net)" "$fixture_case" \
  > "$evidence/stdout" 2> "$evidence/stderr"
result=$?
printf '%s\n' "$result" > "$evidence/exit"
printf '%s\n' "$evidence"
(exit "$result")
```

Record the exact invocation and reviewed source identity with the evidence.
The runner checks the mount, PID, and network namespace identities, empty
runtime directory, absent display and input endpoints, and the command deny
shims before launching MCP. The child uses explicit synthetic session
variables and a private D-Bus configuration without service activation.
Inspect the recorded namespace identities, fake service owners, preflight
denials, and server denials. A shim hit on ydotool, xdotool, or wtype is a
fixture failure.

In each cancellation case, the fake service records the selected barrier
and holds the reply or signal. The client sends `notifications/cancelled`
for the original numeric request ID, then sends `ping` on the same stream.
The pinned rmcp 2.1.0 stdio reader processes one newline-delimited message
at a time, and its service loop applies cancellation before handling the
next message. A successful pong while the reply remains held establishes
the ordering for this version. Recheck that property if the MCP dependency
or transport changes.

For the cases that cancel before dispatch, exit `1` with
`cancelled_type_text_dispatched_keyboard` identifies keyboard dispatch after
ordered cancellation. It requires a passing uncancelled control, a witnessed
barrier, the original clipboard, and a successful follow-up Escape on the
same connection. Exit `0` requires zero keyboard notifications for the
cancelled request, an unchanged synthetic field, the original clipboard,
no held keys, and the successful follow-up.

For `cancel-dispatched`, exit `0` requires one complete chord with reverse
key release, a single prepared write and restoration, and the queued Escape
after restoration. The exact keyboard sequence rules out a second paste
attempt in this fixture. A nonzero exit outside the classified dispatch
failure supplies no passing evidence. Keep the original prepared-write RED
directory and its recorded fixture digest when running a successor.

The fake services do not expose an observable wait at the inner portal
input lock or control the private bus daemon's portal-owner lookup reply.
The portal Start controls cover session setup. The prepared-write and
first-key controls cover the corresponding transport replies. The inner
waits and native KDE behavior require separate evidence.
