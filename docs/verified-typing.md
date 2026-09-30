# Verified raw typing

Automatic raw `type_text` fallback verifies the daemon's actual virtual keyboard
and compares the resolved CLI's key sequence with the requested text. A stock
ydotoold socket or a matching device name does not establish that association.
When qualification is unavailable or the sequence differs, the request stops
before raw input. Explicitly forced ydotool typing retains its layout-dependent
compatibility behavior.

Set `COMPUTER_USE_LINUX_YDOTOOL_IDENTITY_SOCKET` only for a compatible producer
implementing the separate interface below. The consumer uses the selected
`YDOTOOL_SOCKET`, fingerprints the resolved supported CLI, and captures its
strokes in a private datagram sink. The CLI is trusted operator equipment;
capture is not an executable sandbox. The consumer implements the wire contract
independently. The optional producer extension and its adoption are separate
from the MIT consumer; the installer does not provision it.

## Supported native profile

Qualification is fresh for each request and currently supports:

- Explicit `XDG_SESSION_TYPE=x11`, no `WAYLAND_DISPLAY`, and a local native Xorg
  process in the same PID namespace. XWayland is unqualified.
- Printable ASCII with at most 4096 bytes, captured as one ordinary or Left Shift
  stroke per character. The actual CLI sequence must produce each requested
  character in both the slave and paired master maps.
- Exactly one enabled libinput slave keyboard for the producer's sysfs event
  device, with verified standard evdev translation and XI key classes. Identical
  device names do not establish identity. Hybrid evdev pointer devices are
  outside this profile.
- One group, simple supported key types and actions, neutral initial modifiers,
  locks, and held keys, and physical Left Shift producing only Shift. Reachable
  dead or Compose symbols, unmodeled actions, and accessibility controls refuse
  qualification even for a plain-letter request.
- Only RepeatKeys may be enabled among XKB boolean controls, and per-key repeat
  must be disabled for every used key on both slave and master. A minimum key
  hold cannot establish a maximum hold under scheduling delays. Disabling global
  repeat alone does not satisfy this condition.
- Concrete paired-master and core focus within the requested native window and
  PID ancestry. Independent slave focus must be None or the same concrete focus;
  PointerRoot remains unqualified because it can deliver under the pointer.

These restrictions can exclude a stock Xorg profile. The server does not change
keyboard controls, focus policy, or maps to qualify a request. Prefer a literal
backend when the profile is unsupported.

The observer rechecks device identity, map, controls, state, and focus before
each stroke and monitors input and changes through completion. A slave switch
can produce DeviceChanged and stop the request after partial submission. Any
unexpected change or cancellation stops subsequent strokes and finishes owned
key cleanup. Input already submitted cannot be undone: inspect the application
before retrying. Kernel ACKs and observed strokes establish submission, and
application readback establishes insertion.

## Producer wire contract

All integers are little-endian. The identity listener is AF_UNIX SOCK_SEQPACKET
and is separate from the legacy raw datagram socket. Never probe a raw socket
with new protocol bytes. Its permissions and ACL must grant no broader access
than the raw endpoint. Success transfers one CLOEXEC SOCK_SEQPACKET channel with
SCM_RIGHTS. Both endpoint peers must identify the same daemon in the same PID
namespace; the configured raw socket's filesystem device and inode must match.

| Identity request, 16 bytes | Value |
| --- | --- |
| 0–7 | `YDOTID1\0` |
| 8–9 | Version 1 |
| 10–11 | Open operation 1 |
| 12–15 | Length 16 |

| Identity response, 96 bytes | Value |
| --- | --- |
| 0–7, 8–9, 12–15 | Magic, version 1, length 96 |
| 10–11 | Status: ready 0, busy 1, unsupported 2, internal 3 |
| 16–31 | Random daemon/device lifetime instance |
| 32–63 | NUL-padded UI_GET_SYSNAME from the daemon's actual uinput descriptor |
| 64–71, 72–79 | Raw socket filesystem device and inode |
| 80–83, 84–87 | Maximum strokes 4096, printable-stroke feature 1 |
| 88–95 | Reserved zero bytes |

An error carries no descriptor and zeros all fields after the header. Success
carries exactly one descriptor. Truncation, extra descriptors, padding, reserved
bits, peer mismatch, and socket replacement are refused.

Channel requests are 16 bytes: magic `YDP1` (0–3), operation (u16 at 4), flags
(u16 at 6), sequence (u32 at 8), Linux keycode (u16 at 12), and zero reserved
bytes (14–15). Operations are stroke 1, finish 2, and cancel 3. Sequences start
at 1 and increase by exactly one. Stroke flag bit 0 means Left Shift; remaining
bits are zero. Printable keycodes are 2–13, 16–27, 30–41, 43–53, and 57. Finish
and cancel have zero flags and keycode.

Replies are 16 bytes: magic, operation OR `0x8000`, status, sequence, and u32
error. Asynchronous revocation uses operation `0xffff`. Status is success 0,
protocol error 1, revoked 2, or kernel error 3. Error codes are malformed 1,
sequence 2, key/flags 3, overlapping stroke 4, limit 5, legacy traffic 6,
timeout 7, disconnect 8, and submission/release failure 9.

The daemon owns press, at least 20 ms hold, reverse release, and SYN_REPORT for
each stroke; it acknowledges after kernel writes. Only one stroke is outstanding.
The consumer adds a 20 ms intercharacter delay. Legacy traffic revokes the
verified channel before legacy dispatch. Failed release destroys or invalidates
the device and ends its use. Bounds are 2 seconds for handshake/ACK, 10 seconds
idle, 300 seconds overall, 4096 strokes, and 2 seconds for consumer cleanup.

## Qualification evidence

Constructed protocol fixtures establish consumer and cleanup behavior. Use an
isolated native-Xorg guest with its own kernel and devices for association and
readback. `scripts/native_keyboard_test.py` exercises the public MCP action and
an owned Qt field; it requires explicit guest opt-in and a matching provisioned
cloud instance. Record the source/binary identity, guest-only control changes,
device and focus identities, requested text, refusal or actual value, and
cleanup. No host daemon or input-seat changes are part of that test.
