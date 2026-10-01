# Issue tracker: GitHub

Issues and specs for this repository live in GitHub Issues for
`nisavid/computer-use-linux`.

## Repository boundary

These rules govern agent-, skill-, and operator-issued tracker operations from
a checkout. Checked-in automation may use repository context supplied by its
execution environment when that repository-relative behavior is part of the
workflow contract.

- Pass `--repo nisavid/computer-use-linux` to every such `gh issue` command.
- Pass an explicit `--repo` to every `gh pr` command: use
  `nisavid/computer-use-linux` for fork-local pull requests and
  `agent-sh/computer-use-linux` for upstream contribution pull requests.
- Use explicit `repos/nisavid/computer-use-linux/...` paths for tracker-related
  `gh api` calls.
- Never rely on GitHub CLI repository inference for agent, skill, or interactive
  operator commands in this checkout.
- Operations against `agent-sh/computer-use-linux` require separate, explicit
  direction. This tracker configuration does not authorize upstream issues.

## Conventions

- **Create an issue**:
  `gh issue create --repo nisavid/computer-use-linux --title "..." --body "..."`
- **Read an issue**:
  `gh issue view <number> --repo nisavid/computer-use-linux --comments`
- **List issues**:
  `gh issue list --repo nisavid/computer-use-linux --state open`
- **Comment**:
  `gh issue comment <number> --repo nisavid/computer-use-linux --body "..."`
- **Apply or remove labels**:
  `gh issue edit <number> --repo nisavid/computer-use-linux --add-label "..."`
- **Close**:
  `gh issue close <number> --repo nisavid/computer-use-linux --comment "..."`

## Pull requests as a triage surface

**PRs as a request surface: no.**

## Checked-in automation

`.github/workflows/upstream-drift.yml` owns at most one open issue, identified by
the `<!-- upstream-drift-tracker -->` marker in its body:

- When the fork becomes drifted, it opens the issue with `needs-triage` and
  mentions the repository owner.
- While the fork stays drifted, it rewrites the title and body on each run. It
  comments, mentioning the owner, only when a new stable upstream release
  appears.
- When the fork is no longer drifted, it closes the issue as completed. A later
  drift opens a new issue.
- It never changes labels after creation, assigns, or links issues.

Triage the drift issue like any other; removing `needs-triage` does not stop the
updates. Do not edit its marker lines or open a second drift issue by hand.

## Skill operations

When a skill says to publish to the issue tracker, create an issue in
`nisavid/computer-use-linux`.

When a skill says to fetch a ticket, read it from
`nisavid/computer-use-linux`.

## Wayfinding operations

- **Map**: Create one issue labelled `wayfinder:map`.
- **Child ticket**: Create an issue labelled `wayfinder:research`,
  `wayfinder:prototype`, `wayfinder:grilling`, or `wayfinder:task`, then link it
  to the map through GitHub's sub-issues API.
- **Blocking**: Use GitHub's native issue dependencies. Pass the blocker's
  numeric database ID—not its issue number—to
  `repos/nisavid/computer-use-linux/issues/<child>/dependencies/blocked_by`.
- **Fallback**: If sub-issues or dependencies are unavailable, use a task list
  in the map and a `Blocked by:` line in each child.
- **Frontier**: Select the first open child that has no open blocker and no
  assignee.
- **Claim**: Assign the ticket to `@me` before beginning work.
- **Resolve**: Post the answer, close the ticket, then add a short linked
  context pointer to the map's Decisions-so-far section.

Every agent-, skill-, and operator-issued issue command and issue-related API
request must explicitly target `nisavid/computer-use-linux`. `gh pr` commands
must follow the repository rules above.
