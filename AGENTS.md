# AGENTS.md

`CLAUDE.md` links here so agent harnesses share one instruction source.

## Project

`computer-use-linux` is a Rust MCP server and CLI for observing and controlling
Linux desktops.

## Agent skills

### Issue tracker

Engineering skills publish issues and specs to GitHub Issues for
`nisavid/computer-use-linux`; scope every tracker operation explicitly to that
repository. See `docs/agents/issue-tracker.md`.

### Triage labels

The triage vocabulary uses the five default engineering-skill labels. See
`docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. See `docs/agents/domain.md`.

### Fork stewardship

This repository is the maintained `nisavid/computer-use-linux` fork of
`agent-sh/computer-use-linux`. Before assessing or synchronizing upstream,
changing carried divergence, or preparing a release, read
`docs/agents/fork-stewardship.md` and `.agents/fork-ops.toml`.

Use `onboarding-forks-for-agent-maintenance` when revisiting the fork's
authority and operating contract. Use `fork-ops` for supported configuration,
capability, and status operations. Do not infer authority for planned Fork Ops
operations that its current capability report does not implement.

After a fresh clone, or before an upstream-track operation when the track is
missing, run `.agents/bootstrap-fork-ops.sh`. It verifies the fork origin,
provisions the read-only `upstream` remote, disables upstream pushes, and
fetches upstream `main`.

## Operating Policy

- This repository uses agentic engineering and operations. Agents should perform assigned tasks autonomously until they reach a boundary that requires stakeholder policy or an unavailable control surface.
- The user reserves authority over project initiatives and over initiation or continuation of work sessions. Within an active user-directed session, agents should drive execution, review loops, commits, publication steps, and cleanup unless escalation is required.
- Escalate when a decision or action impacts stakeholder concerns and the stakeholder's policy is unknown or uncertain.
- Escalate when an action must be taken but the agent lacks an autonomous control surface for it.
- When escalating a decision and a set of plausible, distinct choices is known, use a multiple-choice input tool if one is available in the interactive context. Include a way for the human operator to provide custom input.
- When escalating an action with a known prescribed path, present the steps clearly for the human operator to perform. Prefer fewer steps; present commands in easily copyable blocks, and prefer a single one-line command when practical.
- For every escalation, make the return contract clear: state exactly what result, confirmation, artifact, or output is needed to hand control back to the agent, and make it easy to validate.
- Prefer verified repository facts over guesses or aspirational guidance.
- When adding new agent-facing instructions, ask whether the information is durable, non-obvious, and useful before scouting a task.
- Remove guidance that becomes redundant with ordinary file discovery.
