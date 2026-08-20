# Issue tracker: GitHub

Issues and specs for this repository live in GitHub Issues for
`nisavid/computer-use-linux`.

## Repository boundary

- Pass `--repo nisavid/computer-use-linux` to every `gh issue` command.
- Pass an explicit `--repo` to every `gh pr` command: use
  `nisavid/computer-use-linux` for fork-local pull requests and
  `agent-sh/computer-use-linux` for upstream contribution pull requests.
- Use explicit `repos/nisavid/computer-use-linux/...` paths for tracker-related
  `gh api` calls.
- Never rely on GitHub CLI repository inference in this checkout.
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

Every tracker command and API request must explicitly target
`nisavid/computer-use-linux`.
