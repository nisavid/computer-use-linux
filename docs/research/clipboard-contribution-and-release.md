# KDE paste contribution and release handback

I have a qualified local repair for the Programmer Dvorak paste failure and the selected scope/automatic-raw boundaries at consumer commit `3436cda38d1f6ce1e777186b99c99edbf98aa133`, based on upstream [`418892f10e6840c45d92e4911f499f2e33994c94`](https://github.com/agent-sh/computer-use-linux/tree/418892f10e6840c45d92e4911f499f2e33994c94) (v0.7.7). The tested executable was built at implementation commit `a9967f5e8f15a3faa1e45959f623ee2a51e5c03d`; only the skill's verification-reference routing differs in the final revision. The fresh private KWin result sends Control_L+v and inserts exactly the requested marker. Successful tool responses describe dispatch; the owned application's value supplies insertion evidence. Five bounded procedure cases also pass; the [consumer handoff](local-repair-qualification.md) records their scope. Publication, upstream acceptance, installed projection, and release verification remain pending.

## What PR #25 established

I inspected [PR #25](https://github.com/agent-sh/computer-use-linux/pull/25), its full final patch/review discussion, the landed commit, and subsequent relevant changes through the comparison base. The final revision was `a6a1f6b0a619aaa328fd16257b5445399eaa49d2`; it landed as [`a0c5da2764a2890021c5b0ef9e7c535a681374e7`](https://github.com/agent-sh/computer-use-linux/commit/a0c5da2764a2890021c5b0ef9e7c535a681374e7). Its final patch and landed first-parent diff were byte-identical: 44,542 bytes, SHA-256 `32e8560b1f048f1ef2849d19ab5598b29439a488d1cd8266a814a7f4f3dc3ae2`.

PR #25 made Klipper session calls asynchronous with zbus, preserved trailing newlines, serialized clipboard transactions within one server instance, bounded proxy/method calls, and delayed restoration by 1.5–5 seconds according to text length. It snapshots a text string, not MIME data/history. Its paste chord still used physical Left Control 29 plus US V 47. A clipboard write, delay, or successful portal reply does not acknowledge insertion. [Landed transaction source](https://github.com/agent-sh/computer-use-linux/blob/a0c5da2764a2890021c5b0ef9e7c535a681374e7/src/server.rs).

The same contribution included geometry, cache, action-default, subprocess, and pointer changes. Those accepted changes are already in the comparison base. The useful contribution shape is a focused repair of current behavior with retained regressions, not a wholesale port of the historical branch.

@avifenesh approved the final revision after reporting fmt, tests, Clippy, builds, Node syntax, and MCP safety. Review fixes addressed stale post-activation geometry, delay-dependent timeout budgeting, and bounded proxy creation. These are attributed review reports. The KDE tests exercised delay/timeout calculation and did not inspect insertion in a KWin widget. The live-layout gap remained open despite those checks. [Review discussion](https://github.com/agent-sh/computer-use-linux/pull/25/files).

## Later source behavior

| Contribution | Behavior present at the comparison base | Evidence limit |
| --- | --- | --- |
| [PR #51](https://github.com/agent-sh/computer-use-linux/pull/51) | Portal keyboard sessions can serve Wayland chords; send failure does not replay through ydotool. | Its author lacked a Plasma hardware test. |
| [PR #113](https://github.com/agent-sh/computer-use-linux/pull/113) | Terminal shortcut selection and requested-focus verification after KDE portal setup. | The xterm reproduction establishes why shortcut choice needs application context. |
| [PR #165](https://github.com/agent-sh/computer-use-linux/pull/165) | Ctrl+V, Ctrl+Shift+V, and Shift+Insert choices; app-scoped AT-SPI can distinguish ordinary editable widgets inside terminal targets. | Its author lacked end-to-end KWin/Klipper/portal qualification. |
| [PR #193](https://github.com/agent-sh/computer-use-linux/pull/193) | Non-KDE modifiers/named portal keys use keysyms; letters/digits remain physical. | KDE paste was still physical. The mixed dispatcher provides the narrow semantic shortcut seam. |

## Final selected contract and source shape

The repair range contains separate commits for process-output draining, strict accessibility scope, semantic paste/cleanup, portal cancellation, and verified automatic raw typing. `src/server.rs`, `src/atspi_tree.rs`, `src/remote_desktop.rs`, and the private `src/verified_typing/` module implement the selected behavior. Existing MCP action schemas remain intact.

- Explicit app/window observation scope must resolve. Missing PID/owner information cannot broaden a targeted tree or authorize element mutation. Failed scope clears actionable cache; `perform_action`, `set_value`, and element click/scroll query current ownership against the latest requested scope. Unscoped desktop observation remains supported. Unresolved requested screenshot targets fail before capture.
- KDE paste resolves the selected shortcut through semantic keysyms. Cancellation before dispatch stops the new paste. Owned key/session cleanup finishes before conditional clipboard restoration. Restoration preserves a detected external clipboard change and covers previous text only; it is neither atomic compare-and-swap nor MIME/history restoration.
- Literal fallback after portal setup refocuses and verifies the requested target. A launched backend error or possible submission stops without replay. Explicitly forced raw typing remains a labeled layout-dependent compatibility route, and `press_key` retains its physical-position contract.
- Automatic raw text requires the configured identity endpoint, selected raw socket, fingerprinted supported CLI and captured actual strokes, exact producer-device/libinput association, supported effective slave/master maps and actions, neutral state, disabled used per-key repeat, and verified current native focus/PID/XID. Unknown conditions refuse before raw input; a mismatch is reported as incompatible. Printable ASCII is bounded to 4096 bytes. Wayland/XWayland, stock unsupported controls, reachable dead/Compose symbols, and PointerRoot slave focus are outside this supported profile.

The independent MIT consumer and optional AGPL producer are separate components. The installer can configure stock ydotoold but does not install the verified producer. Trusted CLI capture is not an execution sandbox. Kernel acknowledgements plus observation establish submitted strokes; application readback establishes insertion. Earlier submitted input cannot be undone after cancellation or a native change, so a partial result requires inspection before another mutating request (`docs/verified-typing.md`, `skills/computer-use-linux/references/input-verification.md`).

The native translation/focus assumptions are version-bound to X.Org's official [xf86-input-libinput 1.5.0 source](https://www.x.org/releases/individual/driver/xf86-input-libinput-1.5.0.tar.xz) and [Xorg 21.1.16 source](https://www.x.org/releases/individual/xserver/xorg-server-21.1.16.tar.xz): the driver's evdev-to-X keycode offset is eight, while slave None and PointerRoot have distinct delivery semantics. This source grounding supplements actual association/map/readback evidence; a driver or layout label alone does not qualify input.

## Selected observed evidence

The pristine upstream executable, SHA-256 `f6b5d8bacfcb64cacb0c4e8f2f93689db51f53d3e192bb4ce5d6087e6c1cd363`, reproduced the Programmer Dvorak failure in a private KWin session. The field began as `LEFT_OLD_RIGHT` with `OLD` selected. Targeted `type_text` for `LAYOUT_PROBE_PASTE` returned `ok:true`, emitted Control_L+k, and left `LEFT_OLD`.

The final immutable repair executable, SHA-256 `05754d622928c2be3e36d42343fbad01b4271d88c5f3fa7ff488ef1d4e74c5d9`, has a fresh private Programmer Dvorak result: Control_L+v, `ok:true` with a shortcut-dispatch message, and exact `LEFT_LAYOUT_PROBE_PASTE_RIGHT`. The fixture had its own display, KWin/Plasma, bus, configuration, clipboard, and owned Qt field, with physical input routes disabled and consent confined to that session. AT-SPI was unavailable, so this is portal/clipboard/widget qualification. A Plasma Activity or virtual desktop would not supply the same separate input seat.

I separately ran the final executable through all 13 existing public paste/portal/fallback regressions and all 11 public strict-scope regressions; all passed. The paste cases cover semantic paste, prepared/dispatched cancellation, conditional restoration/external change, write/portal errors, delayed release cleanup, GNOME literal route selection, forced compatibility, consent-start cancellation, missing literal route, raw-probe cancellation, and refocus before wtype after portal denial. Scope cases cover unresolved title/PID/owner, missing window PID, valid/unscoped tree behavior, failed-scope cache, stale/slow ownership, partial coordinate scroll, snapshot/action serialization, and screenshot refusal. These constructed private services observe separate application values/counters and held keys; they are public-interface regressions, not native compositor/device substitutes.

The retained final public native matrix has 13 passing controls in a disposable guest using Xorg 21.1.16, xf86-input-libinput 1.5.0, real producer keyboards, and an owned Qt field. It contains four insertion positives and nine pre-input refusals:

| Native control | Requested text | Observed result |
| --- | --- | --- |
| Supported US profile | `a A!` | Exact insertion. |
| Stock UK map with reachable dead symbols | `a` | Refused; unchanged field, no key events. |
| Normalized two-level UK map | `a A!` | Exact insertion. |
| Same normalized UK map, mismatching captured stroke | `@` | Incompatible; unchanged field, no key events. |
| Stock Dvorak map with reachable dead symbols | `a` | Refused; unchanged field, no key events. |
| Normalized two-level Dvorak map | `a` | Exact insertion. |
| Same normalized Dvorak map, mismatching captured stroke | `q` | Incompatible; unchanged field, no key events. |
| US International reachable-dead map | `a` | Refused; unchanged field, no key events. |
| Caps Lock | `a` | Refused; unchanged field, no key events. |
| Used per-key repeat enabled | `a` | Refused; unchanged field, no key events. |
| Slave PointerRoot focus | `a` | Refused; unchanged field, no key events. |
| Equal-name US/Dvorak devices; selected US association | `a A!` | Exact insertion. |
| Equal-name US/Dvorak devices; selected Dvorak association | `q` | Incompatible; unchanged field, no key events. |

The guest explicitly cleared unsupported controls and used per-key repeat bits and set supported slave focus; the consumer did not perform those changes. The normalized UK/Dvorak positives remove reachable dead/Compose symbols and unsupported type complexity. They establish request-specific matching behavior, not stock UK/Dvorak support or default-repeat safety. The application's pending Compose state is not established from XKB. The general press-key and pointer paths are not covered by this matrix.

The separate native producer cleanup controls pass for client disconnect while Shift is held, daemon termination/device destruction with neutral master state, and same-path raw-socket replacement refusal. The disconnect log observes reverse key releases; daemon termination is a device-destruction/neutral-state result, not an observed key-up write after termination. Producer cleanup is separate from consumer/source review and application insertion.

The selected observer source review is clean on the frozen final bytes, including actual device binding, map/actions/state/repeat, None/equal slave focus, selected slave/master ancestry notifications, exact `_NET_WM_USER_TIME` allowance, completion barriers, and bounded collection. Constructed wire controls and source review establish different facts from the native matrix. The five declared procedure cases have a clean independent application/discovery grade; broader comparative evaluation, installed projection, and downstream lookup remain unqualified.

## Executed checks and contribution expectations

I ran the non-Cargo CI surfaces against the final immutable server: installer syntax and 12 regressions, MCP safety, Node syntax/version equality, wrapper install/help, package packing/check, Zod schema, Pi build/catalog/typecheck, 26 Pi tests, and the installed-Pi lifecycle smoke all passed. Node 24.21.0 and npm 11.20.0 ran from temporary equipment. All 95 captured inputs are byte-identical to the clean repair commit. Potential desktop/device queries ran in private namespaces; lifecycle installs copied the supplied server/helper into temporary prefixes using local overrides and a populated offline cache.

Retained final gate logs show fmt, locked all-target check, warnings-denied Clippy, warnings-denied rustdoc, Agnix 0.56.0, Cargo audit, and Cargo publication dry-run succeeding. The full Rust rerun passed 375 library tests, seven helper tests, and one export test; four opt-in tests remained ignored. The first sandbox run's sole failure was its `/tmp` ownership/path qualification, and the unsandboxed filesystem rerun supplies the full-suite result. The dry run packaged and verified 97 files, emitted the existing-version/cache warnings, and explicitly aborted upload. No crate or npm publication was performed.

These are local workflow checks. They do not establish hosted CI, ARM64, remote release download/checksum behavior, every desktop/application, or installed-host adoption. The existing COSMIC helper was byte-checked for packaging equivalence; its exact build revision and live COSMIC behavior were not independently qualified.

[CONTRIBUTING.md at the comparison base](https://github.com/agent-sh/computer-use-linux/blob/418892f10e6840c45d92e4911f499f2e33994c94/CONTRIBUTING.md) requires fmt, locked all-target check, warnings-denied Clippy, locked tests, installer regressions, MCP safety, and Agnix. It asks for tested desktop/session details and a doctor report for compositor/portal/accessibility issues, preserved safety annotations, updated command docs, focused changes, and conventional prefixes when practical. The inspected policy contains no DCO requirement. Package changes add publication dry-run and npm packing; hosted CI adds audit, docs, schema, dependencies, and Pi checks. I would accompany the focused contribution with public-safe session/profile details and the readback/refusal evidence rather than claim universal layout support.

## Evidence binding

| Final retained artifact | SHA-256 |
| --- | --- |
| Public regression manifest (13 paste + 11 scope) | `989b5c724ce46a09a7efb532eec51a4f10c948c3aac654b474de6def8fcc011c` |
| Native thirteen-case summary | `4eb75fe8990f343e0c6f5af68e71aafe7bc1f343542c89297828ab121f83ec1b` |
| Fresh private Programmer Dvorak result | `d18c37106d6ded521e469104910e5ab2b49ffb3ffd736f44acd56927a69a63ba` |
| Native producer cleanup result | `035811528b113c161f5ae7d25159c761c7f928f780b632788d1583d5229b8801` |
| Non-Cargo CI manifest | `e38a581652caaad8d6a21aef7e89e73fdcb893d225fc6e5ce1539cf787f6b3fa` |
| Final observer source (`src/verified_typing/x11.rs`) | `1a12fc0ceb14263552f93e871974fac305aa64cc19d48a7e1dfee261a36eac48` |
| Retained implementation source/procedure review manifest | `3f2013e0c75a72ecaf9c559a7cc5f7ec6cfd4b0f71bc30819c27d70d190fc8e0` |
| Final consumer evidence manifest at `3436cda` | `7a8ac8f8a9fc98592084e847d1b9d1a1b277bcfedc3d7ce0af825475aa51f4fd` |
| Optional producer patch / source archive | `749a7602b674bd105628cc42e19d0594959103417d3a7e33f9673036899d543f` / `99057526d0dc9fdd1ea8724a222faa736df532f679830f3910dead8e835f14dc` |

The original PR #25 handback remains historical input. These evidence digests bind retained local results; public evidence publication and permanent links are still pending.

## Release verification still to perform

The compared [v0.7.7 release](https://github.com/agent-sh/computer-use-linux/releases/tag/v0.7.7) was published September 29, 2026 at the upstream comparison commit. The previously inspected [tag workflow](https://github.com/agent-sh/computer-use-linux/actions/runs/36632952353) published GNU Linux x86_64/aarch64 binaries/helpers, checksums, npm, and the crate; the September 30 metadata inspection bound npm gitHead and the non-yanked crate checksum to that release. This is comparison-release metadata, not a claim that the local repair is in a published artifact.

Each Git push and outward upstream write requires separate approval of its concrete destination, revision, text, and evidence. The final procedure has a clean selected application/discovery grade; publication, targeted installation, and downstream lookup still need verification. After upstream integration, record and inspect the landed repair (including any squash), then resolve a release tag, verify source inclusion, workflow/assets/checksums and package versions/registry metadata, checksum the downloaded artifact, and rerun disposable behavior against that artifact. Source acceptance, publication metadata, and observed released behavior must be reported separately. Upstream acceptance and release timing remain maintainer decisions; this handback does not authorize a version bump, separate fork release, producer adoption, or distribution claim.
