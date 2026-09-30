# Desktop action routes and selected qualification

I audited the complete source surface at repair commit `a9967f5e8f15a3faa1e45959f623ee2a51e5c03d`, compared with upstream [`418892f10e6840c45d92e4911f499f2e33994c94`](https://github.com/agent-sh/computer-use-linux/tree/418892f10e6840c45d92e4911f499f2e33994c94) (v0.7.7). The selected repairs require observation scopes to resolve, verify current accessibility ownership before element actions, send KDE paste shortcuts as semantic keysyms, retain cancellation cleanup, and condition automatic raw text on the actual daemon keyboard and request-specific native state. This is a source-wide inventory with runtime qualification for those selected behaviors.

The compared range contains five cohesive commits: bounded process-output draining (`cd057d1`), accessibility scope/ownership (`5adfa0f`), semantic KDE paste and cleanup (`8d7fdd6`), portal cancellation (`e2575d4`), and verified raw typing (`a9967f5`). The manifests still report version 0.7.7. A local repair commit and a version string do not establish distribution of the repair.

## Callable surface

`src/server.rs::ComputerUseLinux::router_with_completion` registers 18 MCP tools by default. `run_shell` requires `COMPUTER_USE_LINUX_ENABLE_SHELL=1`; `complete_interaction` requires `COMPUTER_USE_LINUX_NOTIFY_ON_COMPLETE=1`. The maximum is 20. The Pi extension adds a loader and generated `computer_use_linux_<tool>` wrappers that call the same MCP routes; it introduces no second input engine (`pi/extension/index.ts`, `pi/extension/generated-tools.ts`).

I found no desktop record/replay command in `src/cli.rs`, MCP record/replay route in `src/server.rs`, or recorder/replayer module in the inspected upstream base and repair source. `tests/library_exports.rs::exposes_record_replay_library_surface` checks the exported `snapshot_tree`, screenshot capture, diagnostics, and data types. Its name does not establish a recording or replay implementation. `src/lib.rs` exposes those observation/diagnostics interfaces for external consumers; it does not export an input replay controller. The node cache supports subsequent element targeting, and the shell audit records a digest rather than a replayable command log.

The route descriptions below are source findings. The evidence section identifies the smaller set I executed or checked in retained qualification results; an unqualified route in this inventory is not a claim that a real desktop accepted it.

## MCP route inventory

| Tool | Source route and selection | Meaning of the result |
| --- | --- | --- |
| `doctor` | `diagnostics.rs::doctor_report` probes accessibility, portals, input equipment, and the window registry. | Readiness and available capabilities; no application input proof or request-specific raw qualification. |
| `setup_accessibility` | `diagnostics.rs::setup_accessibility_report` enables the GNOME AT-SPI bridge with `gsettings` and reads settings/readiness. | Configuration operation and readback. Headless regressions do not qualify an installed desktop. |
| `setup_window_targeting` | `gnome_extension.rs::setup_window_targeting_report` writes the bundled extension and enables it through `gnome-extensions`, with a `gsettings` fallback. | File/enable results and reload guidance; not live compositor qualification. |
| `list_apps` | Process candidates plus `atspi_tree.rs::list_accessible_apps`. | Candidate apps and any accessibility error. |
| `list_windows` | `windowing/registry.rs`: GNOME extension, GNOME Introspect, COSMIC helper, KWin, Hyprland, niri, i3, then X11/EWMH. A usable exact-focus X11 result can replace GNOME Introspect's list-only result. | Window records, backend identity, geometry, and backend notes/errors. |
| `focused_window` | Focus query through the selected window backend; COSMIC has a direct query. | Observed focus at query time, not later input delivery. |
| `activate_window` | Resolve selectors, activate through that window's backend, then verify reported focus (`focus_satisfies_target`). No general retry through another compositor backend after selection. | Verified exact-window or allowed app-level focus. |
| `get_app_state` | Resolve requested window/app scope, traverse AT-SPI, optionally capture/crop a screenshot. Unresolved window scope or missing required PID fails without broadening the tree; failed scope clears actionable cached state. Deliberately unscoped observation retains the desktop tree. | Scoped tree, truncation/readiness/errors, optional image and coordinate metadata. An unresolved requested window is rejected before raw screenshot capture. |
| `screenshot` | GNOME Shell, Screenshot portal, native X11 `GetImage`, then `gnome-screenshot`, unless a backend is explicitly pinned. A window target is resolved and normally raised; crop precedes resize. | Captured image, source, dimensions, scale, and format. An unresolved requested target fails before capture; successful capture does not imply accessibility support. |
| `click` | Eligible plain single left element activation prefers a recognized AT-SPI action. Element selection must pass current ownership/scope checks. Explicit coordinates and other click forms use direct uinput absolute pointer, then cached/preferred portal, eligible native-X11 xdotool/XTEST, then ydotool. | AT-SPI action boolean or input dispatch result; direct uinput includes requested/emitted coordinates. No application effect readback. |
| `perform_action` | Resolve cached/indexed/semantic or object-reference target; require latest requested scope membership and a fresh current bus owner; invoke AT-SPI Action. No pointer/key fallback. | Selected action and its boolean result. The existing public schema is retained. |
| `set_value` | Same ownership/scope barrier. AT-SPI Value for applicable numeric values, otherwise EditableText when supported. No synthetic input fallback. | Interface operation result; callers still read the value back. |
| `scroll` | Explicit point, owned element center, or targeted-window center; relative coordinates require a resolved target. Portal, then eligible xdotool wheel-button events, then ydotool wheel events. | Dispatch result and advisory geometry notes, not observed content movement. Element targets receive current ownership checks. |
| `drag` | Direct uinput absolute pointer, then portal, then ydotool move/press/move/release. No xdotool drag route. | Backend operation result; no final UI-state readback. |
| `press_key` | Optional focus verification; Wayland portal chords, eligible X11 xdotool/XTEST, otherwise raw ydotool events. Non-KDE portal modifiers/named keys use keysyms, letters/digits retain physical US positions; KDE's general chord path remains physical. | Deliberate key/chord dispatch. The semantic KDE paste repair does not redefine this compatibility contract. |
| `type_text` | Focus and cancellation barriers. KDE Wayland uses serialized Klipper text preparation plus semantic Ctrl+V, Ctrl+Shift+V, or Shift+Insert through the portal. Other eligible Wayland routes use literal portal keysyms. Literal command selection uses xdotool on native X11 or wtype on compatible Wayland. Automatic raw fallback requires verified daemon/device association and native qualification. Explicitly forced ydotool retains layout-dependent compatibility. | Backend dispatch; verified raw reports observed kernel submission and asks for application readback. No successful response by itself proves insertion. |
| `move_window` | Resolve a window; GNOME extension or X11/EWMH geometry backend. Other registered backends return unsupported. | Re-listed final bounds when available. |
| `resize_window` | Same geometry dispatch; X11 removes maximization, requests wmctrl geometry, and polls the result. | Reported final geometry, including constraints imposed by the WM. |
| `run_shell` (opt-in) | `execute_shell`: same-user `/bin/sh -c`, cleared/allowlisted environment plus explicit variables, canonical working directory, timeout/output caps and process-group cleanup. | Exit status, bounded output, truncation/error, and command audit digest. This is host-code execution, not a sandbox. |
| `complete_interaction` (opt-in) | Best-effort `notify-send` with a two-second bound. | Sent/skipped/error; no proof the user saw the notification. |

Pointer and general keyboard compatibility routes remain outside the new raw-text predicate. A portal dispatch error or a launched command failure is not permission to replay through another backend. Only the documented safe-before-submission paths can proceed to an alternative. Coordinate-only calls retain their existing focus rules; the element ownership checks are not a desktop-wide authorization mechanism.

## CLI, library, installation, and helpers

`src/cli.rs::run_from_env` exposes `mcp`, `doctor`, `setup`, `guard-accessibility`, `apps`, `state [APP_NAME]`, `screenshot`, `windows`, `setup-window-targeting`, and the implemented `abs-test` diagnostic. `abs-test` captures desktop dimensions and directly creates a uinput absolute pointer for one click, bypassing the MCP click dispatcher. I did not actuate that path. `guard-accessibility` is an explicit foreground settings guard, separate from normal input dispatch.

The public library exports diagnostics, accessibility snapshot/data types, and raw screenshot capture (`src/lib.rs`). A library caller has its own invocation and targeting responsibility. The MCP cache and ownership barrier are server behavior, not wrappers around arbitrary library consumers.

`src/bin/computer-use-linux-cosmic.rs` provides probe, window listing/focus, monitor layout, and activation over COSMIC protocols. It is a window-management helper, not a keyboard/pointer injector. `install.sh` can install packages/toolchains/binaries, configure stock ydotoold and uinput permissions, enable GNOME accessibility, and install the extension. It does not provision the optional verified producer. Packaging smoke verifies supplied bytes were copied and executable; it does not qualify live COSMIC or adopt installed-host equipment.

## Accepted selected behavior

Requested observation scope remains binding even when a PID or derived filter cannot be obtained. A target PID cannot fall through to another root by name. Unknown current AT-SPI ownership refuses element mutation; a cached PID cannot replace a failed fresh owner query. Native `perform_action` and `set_value` use the latest snapshot's requested scope without adding public window-target parameters. A failed requested scope leaves no actionable cache; a new valid snapshot can restore it (`server.rs::check_object_ref_target`, `atspi_tree.rs`).

KDE paste uses semantic shortcut symbols rather than US V's physical position. The owned transaction retains input/clipboard locks through cleanup. Cancellation before dispatch sends no new paste. Once dispatch starts, owned key cleanup completes before clipboard restoration. Previous text is restored only if the prepared value remains; a detected external change is preserved. The check/write is not atomic compare-and-swap and restores neither MIME data nor clipboard history. Literal fallback after portal setup refocuses the requested target. A partial or ambiguous result stops without replay.

Automatic raw typing qualifies each request against the selected producer's actual sysfs event node, exact enabled libinput slave keyboard and attached master, resolved CLI fingerprint and captured strokes, effective maps/types/actions, neutral state, controls/repeat, and current focus/PID/native XID. Names and a stock raw socket are insufficient. Qualification requires explicit native X11, printable ASCII up to 4096 bytes, the supported simple one-group profile, no reachable dead/Compose symbols, and used per-key repeat disabled on slave and master. Independent slave focus must be None or the same concrete focus; PointerRoot, XWayland, Wayland, hybrid pointer devices, and unknown conditions refuse. The server changes none of these conditions to qualify a request (`docs/verified-typing.md`, `src/verified_typing/`).

The observer monitors through completion, rejects foreign/unexpected events and identity/map/state/focus/structure changes, and bounds collection. The narrow genuine NEW_VALUE `_NET_WM_USER_TIME` allowance applies only to the cached atom on captured ancestry; owner/unknown/delete/synthetic/helper-window changes remain refused. A failed/cancelled request may already have submitted text: cleanup cannot undo it. The trusted CLI capture is not an execution sandbox, and the independent MIT consumer does not include the optional AGPL producer's implementation or tables.

## Selected runtime evidence

I ran all 13 existing public paste/portal/fallback controls and all 11 public strict-scope controls against the immutable combined executable; all passed. These use private namespaces, fake D-Bus/AT-SPI/backend services, and separate application/clipboard/held-key observations. They establish the selected public protocol, cache, refusal, cancellation, refocus, and cleanup behavior. They do not establish real compositor or kernel-device behavior.

The retained native public-MCP matrix contains 13 passing cases in a disposable native-Xorg guest with an owned Qt field and real producer devices: four exact insertions and nine refusals with no field change or key events. US `a A!` succeeds in the explicitly supported profile. Normalized two-level UK `a A!` and Dvorak `a` succeed, while UK `@` and Dvorak `q` mismatches refuse. Stock UK, stock Dvorak, and US International reachable-dead maps refuse even plain `a`. Caps Lock, enabled per-key repeat, and slave PointerRoot refuse. Two equal-name keyboards select by actual association: the matching US device succeeds while the incompatible Dvorak device refuses. The guest uses Xorg 21.1.16 and xf86-input-libinput 1.5.0; qualification relies on the actual device, not global driver presence. The matching normalized maps are constructed profiles, not stock UK/Dvorak support.

The native producer cleanup log separately shows reverse key releases after client disconnect, neutral master state after daemon termination/device destruction, and refusal of same-path raw-socket replacement without transferring a channel. Those are producer controls, not proof that every consumer cancellation or crash was reproduced natively.

The fresh private KWin/Plasma Programmer Dvorak result shows the final executable emitting Control_L+v and changing the selected field exactly from `LEFT_OLD_RIGHT` to `LEFT_LAYOUT_PROBE_PASTE_RIGHT` for `LAYOUT_PROBE_PASTE`. It used a separate display/compositor/bus/clipboard and owned application with physical backends disabled. AT-SPI was unavailable, so this qualifies portal/clipboard/widget behavior separately from the scope fixture. The pristine upstream executable had emitted Control_L+k and left `LEFT_OLD` while reporting success. The general `press_key` physical route remains outside this result.

## CI evidence and practical limits

I ran the non-Cargo workflow surfaces using Node 24.21.0, npm 11.20.0, the supplied server, and temporary dependencies/prefixes: all 12 installer regressions, MCP safety (18 default, 19 with shell), Node syntax/version equality, wrapper install/help, package packing/check, Zod validation of 18 tools, Pi build/catalog/typecheck, 26 Pi tests, and the one installed-Pi lifecycle smoke passed. All 95 captured source inputs match the clean repair revision byte-for-byte. Potential server/doctor/shell execution ran in private namespaces without host desktop/device access. Bootstrap used locked `npm ci --ignore-scripts`; lifecycle smoke used the task cache and local binary overrides.

The retained final Rust results show fmt, locked all-target check, warnings-denied Clippy, rustdoc with warnings denied, and publication dry-run packaging/verification succeeding. The initial sandbox test failed because its private `/tmp` ownership did not meet the probe's path policy; the full rerun outside that filesystem sandbox passed 375 library tests, seven helper tests, and one library-export test, with four opt-in tests ignored. Agnix 0.56.0 reported no issues; Cargo audit scanned the lockfile without reported vulnerabilities. The dry run packaged 97 files and explicitly aborted upload. No publication occurred.

These local checks do not replace hosted CI or qualify ARM64, remote release downloads, every compositor, pointer landing, geometry, notification visibility, application Compose state, or installed-host adoption. The packaging smoke used a supplied COSMIC helper whose exact build revision was not independently established; byte-copy equivalence is the supported claim.

## Evidence binding and remaining work

| Retained evidence | SHA-256 |
| --- | --- |
| Combined public server | `05754d622928c2be3e36d42343fbad01b4271d88c5f3fa7ff488ef1d4e74c5d9` |
| `scripts/paste_backend_test.py` | `d3ae08088bcb46794da0c144bd7f6cd6d22f14531f923b716c6420c90fdeb7b0` |
| `scripts/accessibility_scope_test.py` | `d0bc942ca25a5bb2fe0f14739394efaa0b6c89b079f51471807f914f207dc081` |
| `scripts/native_keyboard_test.py` | `67c3f0344885db671c0bb9c7e320392ca40a3c0ed64906f196ed9d33403e10bd` |
| Thirteen-case native summary | `4eb75fe8990f343e0c6f5af68e71aafe7bc1f343542c89297828ab121f83ec1b` |
| Fresh private Programmer Dvorak log | `d18c37106d6ded521e469104910e5ab2b49ffb3ffd736f44acd56927a69a63ba` |
| Native producer cleanup log | `035811528b113c161f5ae7d25159c761c7f928f780b632788d1583d5229b8801` |
| Public regression manifest | `989b5c724ce46a09a7efb532eec51a4f10c948c3aac654b474de6def8fcc011c` |
| Non-Cargo CI manifest | `e38a581652caaad8d6a21aef7e89e73fdcb893d225fc6e5ce1539cf787f6b3fa` |

The selected observer/procedure source review is clean for the frozen dependencies; source review and executed native results are separate evidence. Full procedure evaluation remains pending, as do approval and publication of the concrete contribution/evidence, upstream acceptance, and later release verification. The original inventory is retained as a historical input; this draft does not establish a published artifact or completed distribution.
