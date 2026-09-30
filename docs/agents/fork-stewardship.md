# Fork stewardship

## Current policy

`nisavid/computer-use-linux` is a maintained fork of
`agent-sh/computer-use-linux`. The current synchronization baseline is upstream
`main` at `418892f10e6840c45d92e4911f499f2e33994c94` (v0.7.7).

The `nisavid/input-assumptions` candidate carries the selected desktop input
repairs from consumer revision `3436cda38d1f6ce1e777186b99c99edbf98aa133`:
bounded process-output draining, strict accessibility scope and ownership,
semantic KDE paste with cancellation cleanup, portal cancellation, and
request-specific verified automatic raw typing. It also carries the associated
tests, protocol documentation, skill guidance, and research handoff in
`docs/research/`. See [the qualification handoff](../research/local-repair-qualification.md)
for the supported profile and remaining publication/adoption gates. These local
commits do not establish an installed repair or a released artifact.

The fork's agent-maintenance infrastructure remains:

- `.agnix.toml`
- `.agents/fork-ops.toml` and `.agents/bootstrap-fork-ops.sh`
- `AGENTS.md` and its `CLAUDE.md` compatibility link
- `docs/agents/`

Update the baseline and this inventory when an accepted upstream sync or a
fork-local product change alters either statement.

## Authority

Use these sources in descending order:

1. Explicit user direction for the active task.
2. Current upstream source, documentation, releases, and maintainer guidance
   for product behavior.
3. Fork-local `AGENTS.md`, `.agents/fork-ops.toml`, and `docs/agents/` policy.
4. Source structure and tests, with important inferences labelled.
5. Prior agent notes only after checking them against current repository state.

Escalate when these sources conflict or a consequential stakeholder policy is
unknown. Record durable fork-policy decisions here and keep the machine-readable
contract aligned.

## Local bootstrap

After a fresh clone, or before an upstream-track operation when
`upstream-main` is missing, run:

```sh
./.agents/bootstrap-fork-ops.sh
```

The script verifies that `origin` names this fork, creates or verifies the
`upstream` remote, sets its push URL to `DISABLED`, fetches upstream `main`, and
verifies the remote-tracking ref. It is idempotent. Stop on an identity mismatch
instead of rewriting an unexpected remote.

## Change targets

- Make product and workflow changes in the fork by default.
- Create engineering issues and Wayfinder tickets only in
  `nisavid/computer-use-linux`.
- Contribute to upstream pull requests or upstream issues only with explicit
  direction for that operation.
- Preserve upstream crate, npm package, binary, extension, and public URL
  identities unless an explicit rebranding task changes that policy.

## Upstream synchronization

Before a broad sync, fetch `origin` and `upstream`; verify current refs,
worktrees, local changes, unpushed commits, and the divergence inventory. The
`upstream` push URL must remain `DISABLED`.

Preserve upstream commit identity when carrying upstream history into the fork.
Use a normal merge commit for a broad sync; do not rebase or squash upstream
history, force-push it into place, or use a forced repository-sync operation.
This rule governs broad upstream synchronization. Fork-local topic pull
requests may still use the repository's normal rebase-merge preference.

The Fork Ops `upstream-main` track is for current upstream inspection. It is not
an autonomous sync authorization. A broad sync still requires explicit task
direction, an updated divergence assessment, and normal review and publication
gates.

## Releases and publication

The fork has no independent release channel. Do not create or push version
tags, publish fork GitHub releases, or publish to crates.io or npm without
explicit direction. Keep the versions in `Cargo.toml` and `package.json`
equal, and treat a release tag, binaries, checksums, and package artifacts as one
reviewed release unit.

## Generated and runtime boundaries

Generated local artifacts include `target/`, `dist/`, and ignored platform
binaries under `npm/bin/`. Their source owners are the Rust and npm manifests,
release workflows, installer scripts, wrapper code, and GNOME extension source.

Installation can write the user binary directory, GNOME extension directory,
user systemd configuration, and the ydotool socket under the user's runtime
directory. The installer may also request privileged dependency installation or
udev configuration. Keep those privileged steps explicit and user-mediated.

## Validation

Match validation to the affected surface. The hosted baseline includes Rust
formatting, build, Clippy, tests, documentation and package checks; installer
regressions; MCP safety checks; Agnix; npm wrapper and version checks; schema
validation; and dependency auditing. Live desktop behavior additionally needs
the compositor-specific matrix described by the active task or maintainer.
