# Local repair qualification and consumer handoff

I have a locally qualified consumer repair at
`3436cda38d1f6ce1e777186b99c99edbf98aa133`, based on upstream v0.7.7 commit
[`418892f10e6840c45d92e4911f499f2e33994c94`](https://github.com/agent-sh/computer-use-linux/tree/418892f10e6840c45d92e4911f499f2e33994c94).
It repairs semantic KDE paste, requested accessibility scope and current
ownership, cancellation cleanup, literal fallback focus, and conditional raw
typing. The maintained `computer-use-linux` skill now requires its input
verification reference before desktop text steps, including a check of an
already correct value.

The [desktop action audit](desktop-action-audit.md) covers the full source
surface. The [contribution and release handback](clipboard-contribution-and-release.md)
explains the prior clipboard contribution and the remaining upstream path.
This handoff supplies the selected local evidence and the procedure a consumer
must load. Publication, installed projection, downstream lookup, upstream
acceptance, and released-artifact qualification remain separate gates.

## Consumer entry point

Load [computer-use-linux](../../skills/computer-use-linux/SKILL.md) from the
reviewed revision before dependent execution. For desktop text work and
input-failure diagnosis, load
[input verification](../../skills/computer-use-linux/references/input-verification.md)
before the numbered desktop-control steps. Record the source revision and
verify the projection's bytes; an installed version string or a successful
skill catalog listing cannot establish that this reviewed guidance was loaded.

The procedure requires a resolved requested scope, a verified intended target,
inspection of the selected value and surrounding text, and independent
application readback after dispatch. Successful transport describes dispatch,
not insertion. If input may have been submitted, inspect the actual value and
held-key state before any further mutation; do not replay an ambiguous request.
An already correct value still requires scoped observation and readback, with
no new input. Qualification fixtures use their own input seat, bus, writable
configuration, clipboard, and owned applications.

The source hashes are:

| Maintained source | SHA-256 |
| --- | --- |
| `skills/computer-use-linux/SKILL.md` | `d8dc3fcc96f7d8498d56d40855bb7294bd2c8336f4a22282ed08cdfea7bb44d6` |
| `skills/computer-use-linux/references/input-verification.md` | `af2220361d3e2d9360e1d93cb77a74b1c1dd2c8346bb651d62ec31f835a64680` |

The actual npm and Cargo dry-run archives contain those exact bytes. No
installation or projection into the operator's active harness is claimed.
The downstream local-repair and release-adoption tasks retain their own
instructions and acceptance decisions; this work does not alter them.

## Supported input and cleanup contract

KDE paste sends the selected shortcut as semantic keysyms. Cancellation before
keyboard dispatch stops the new paste; owned cleanup finishes after dispatch
starts without replaying input. Previous clipboard text is restored only if
the prepared text remains. A detected external replacement is preserved.
Restoration has a check/write race and does not restore MIME data or history.
Literal fallback rechecks requested focus after portal setup. `press_key`
retains its physical-position behavior, and explicitly forced ydotool remains
a documented layout-dependent compatibility route.

Automatic raw typing qualifies the actual captured sequence for each request.
It requires the verified producer/device association, a local native Xorg in
the same PID namespace, the associated enabled libinput slave and paired
master, verified focus/PID/XID, supported effective maps/actions, neutral
modifiers/locks/held keys, disabled used per-key repeat, and the bounded
printable-ASCII profile. A device name, layout name, or stock daemon socket is
insufficient. Wayland/XWayland, reachable dead/Compose symbols, PointerRoot
slave focus, and unknown conditions refuse this route. Observation continues
through completion; a later change can leave partial input that requires
readback. The application’s pending Compose state is not established by XKB.
See [the protocol and profile](../verified-typing.md).

The optional producer is a separate AGPL prototype. The MIT consumer contains
no copied producer implementation or tables, and the installer does not
provision that producer. Local guest evidence grants no producer adoption,
distribution, or host-installation authority. Trusted CLI capture is not an
arbitrary-executable sandbox.

## Backend and package evidence

The tested server was built at implementation commit
`a9967f5e8f15a3faa1e45959f623ee2a51e5c03d`, SHA-256
`05754d622928c2be3e36d42343fbad01b4271d88c5f3fa7ff488ef1d4e74c5d9`.
Only the skill's reference-routing paragraph differs in the final consumer
revision; production code, Cargo inputs, fixtures, and protocol reference are
byte-identical. I retain that binding instead of describing a new build or
test run on the later documentation commits.

| Executed surface | Observed selected evidence |
| --- | --- |
| Public MCP/private backend fixtures | All 13 paste/portal/fallback and 11 scope controls pass, with separate field, clipboard, cache/action, and held-key observations. |
| Disposable native-Xorg guest | All 13 selected cases pass: four exact insertions and nine pre-input refusals with unchanged field and no key events. Equal-name keyboards are selected by actual device association. |
| Private KWin/Plasma Programmer Dvorak | Semantic Control_L+v inserts exactly `LEFT_LAYOUT_PROBE_PASTE_RIGHT`; the upstream executable emitted Control_L+k and left `LEFT_OLD` while reporting success. |
| Native producer cleanup | Client disconnect produces reverse releases; daemon termination destroys the device and leaves neutral master state; raw-socket replacement refuses a new channel. |
| Rust/local workflow checks | Formatting, locked all-target check, warnings-denied Clippy/rustdoc, 375 library tests, seven helper tests, and one exports test pass; four opt-in tests remain ignored. Installer/MCP/schema/Pi/wrapper checks pass on unchanged implementation inputs. |
| Final package refresh | Fresh npm version/content/packing checks include all 15 intended files. Cargo dry run packages 97 files and verifies compilation, then explicitly aborts upload. Versions remain 0.7.7. |

The native guest used Xorg 21.1.16, xf86-input-libinput 1.5.0, and real producer
devices with an owned Qt application. It explicitly configured supported
controls, disabled used per-key repeat, and set supported slave focus. The
consumer did not change those conditions. UK/Dvorak insertion positives use
constructed two-level maps; stock UK/Dvorak with reachable dead symbols refuse
even plain `a`. These results do not establish stock-layout support.

Retained local evidence is bound by SHA-256:

| Evidence | SHA-256 |
| --- | --- |
| Public regression manifest | `989b5c724ce46a09a7efb532eec51a4f10c948c3aac654b474de6def8fcc011c` |
| Native matrix summary | `4eb75fe8990f343e0c6f5af68e71aafe7bc1f343542c89297828ab121f83ec1b` |
| Private Programmer Dvorak result | `d18c37106d6ded521e469104910e5ab2b49ffb3ffd736f44acd56927a69a63ba` |
| Final npm package manifest | `7fa0797e80f634f3e8de8a7d936a62fe24e697c74c8e4612d93d8b6b4b0933ad` |
| Actual npm archive | `d23cbe23f4c1809a5ea7a4ae799f21bf8cde1a6094f5ae9c593bd62989ace28b` |
| Final Cargo dry-run log | `77eea45a6401055986b33d9bfadbba6170f6d17d3448caf99682b15195e6b6a8` |
| Actual Cargo dry-run archive | `ce33d4901b64a6436c2433c4489fbd0d9e2611632106dd75d6c9db758f0f161d` |

The local evidence and fixture captures remain retained separately; this
document does not publish private desktop captures or authentication files.
The hidden private desktop and disposable guest processes are stopped. The
operator's portal configuration and installed desktop were not changed.

## Procedure application and discovery evidence

I used the operator-approved independent-review and actual plugin-eval loop
for this increment because `ralph-review-until-clean` was unavailable. The
final five-case batch used plugin-eval 0.1.2 with native Codex CLI 0.147.0,
model `gpt-5.6-luna`, and fresh private executor workspaces. The three desktop
cases explicitly invoked the provisioned immutable skill; configuration and
headless cases tested implicit discovery separately.

| Case | Independent result |
| --- | --- |
| Selected replacement | `LEFT_DONE_RIGHT`, neighboring text preserved; full skill/reference load before scoped observation, intended input, and independent readback. |
| Partial dispatch | `LEFT_PRO_RIGHT`, actual possible-submission error, later readback, and no retry or replay; the result reports partial/ambiguous input. |
| Already correct value | Value, selection, and focus unchanged; full skill/reference load before scoped observation and readback; no mutation. |
| MCP configuration discovery | Actual skill loaded before creation of the `linux-desktop` stdio entry; no server launch, install, or desktop action. |
| Headless JSON negative | JSON values/types preserved with two-space formatting; no skill/reference load or desktop action. |

The independent grader reconciles final state, nested completed executor
items, and fixture events. All five declared cases and its 20 controls pass.
Two grader false negatives were repaired against the same retained traces:
an accurate “ambiguous” partial report and a filename exclusion mistaken for
a configuration write. The initial report and all earlier missing-reference
or broken-executor runs remain non-qualifying evidence. No model rerun was
used to correct those grader findings.

The final grade is SHA-256
`2b8904c3e19b527fffa1399a708074388298e3e50a15e4759e1ac4db2f75d589`;
the grader is `eaaec26e0600e33708064ddf3fee257e2d0348858542e74419fb9ca89cdef8dc`.
The constructed shell/JSON fixture is model-visible, not an adversarially
tamper-proof oracle. Native catalog preflight establishes availability, not
invocation. One configuration-discovery positive does not establish general
automatic GUI invocation. These procedure cases do not replace public/native
backend tests or the broader controlled comparative source-stage evaluation.

### Evaluator findings

The observed-usage analysis reports **77/C/high**, one failure, and two
warnings. I retain that result rather than calling it a zero-finding report.
Independent inspection of plugin-eval's implementation and raw executor
usage supports these dispositions:

| Finding | Disposition for this increment |
| --- | --- |
| `observed-usage-estimate-drift` | Rejected as a skill-cost defect: the evaluator compares 2598 estimated trigger/invoke tokens with 95,665.8 average whole-turn input tokens, without skill attribution or a matched no-skill control. The headless negative never loads the body but records 47,432 input tokens. The arithmetic is valid; the attribution is not established. |
| Extra frontmatter keys | Rejected as a compatibility blocker: inherited `author`, `platforms`, and `compatibility` metadata falls outside this evaluator's common-key allowlist, while native catalog discovery and candidate loading succeed. Cross-harness metadata cleanup is separate work. |
| Heavy invoke estimate | Retained as a size warning: the static invoke estimate is 2539 tokens for the complete `SKILL.md`, compared with 2459 upstream. It is a heuristic size comparison, not a measured regression in marginal invocation cost or a violated acceptance threshold. Broader text reduction and controlled cost comparison remain follow-ups. |

The usage normalizer also omits the native `cached_input_tokens` field; the
five raw turns average 79,462.4 cached input tokens. Subtracting cache would
still not isolate skill cost. The official tool/shell counters miss this
executor's nested command items, so the independent grader uses retained
completed items and observed fixture effects. No evaluator output, model
evidence, or score was edited to obtain a pass. The selected application and
discovery criteria have a clean latest grade; causal efficiency and general
automatic invocation are not claimed.

## Remaining review and adoption steps

The selected implementation and procedure source reviews are clean on the
final consumer bytes. The advisory risk assessment recommends human review
for the high-impact desktop input boundary; it grants no publication or merge
authority. Hosted CI, GNOME/COSMIC runtime, ARM64, arbitrary editor/IME behavior,
and exhaustive live mid-stroke changes remain outside this qualification.

Each proposed Git push needs its own approval. Upstream-facing text must be
prepared from the pushed immutable diff and approved separately. A consumer
then needs an authorized installation/projection and verified lookup of the
reviewed procedure before dependent execution. Released-artifact adoption
requires source inclusion, package/workflow/checksum verification, and a fresh
disposable behavior check against that released artifact.
