# Resolve a verified upstream release input

This procedure observes latest stable upstream for comparison. Its record does
not establish that the selected commit is incorporated into a fork candidate.
For release provenance and a known historical incorporated baseline, use the
separate verification path in `docs/agents/release-preparation.md`. Preserve this
selector and the existing drift entry points when developing release delivery.

From an isolated checkout with the configured read-only upstream remote and no
concurrent fetches, use Git, authenticated GitHub CLI, and Python 3.11 or newer.
Choose a fresh output path in a directory you own.

```sh
bash .agents/upstream-drift.sh --release-input /path/to/new-release-input.json
```

The resolver selects GitHub's latest stable release, validates its tag ref
format, and fetches it into `FETCH_HEAD`. It then reads the release and tag ref,
verifies the fetched object and peeled commit, and checks the package version in
that commit's `Cargo.toml`. The release identity and notes must match the initial
selection. It reads the release and ref a second time before
creating the record. The output includes the exact notes
body, its UTF-8 SHA-256, the Cargo blob's SHA-256, and the resolver and entry
point digests. An existing path causes the write to fail. A different latest
release needs a new output path.

Retain the record, its SHA-256, the source revision, this procedure's digest,
the command, and its original exit and diagnostics. GitHub and Git do not
provide one atomic snapshot across these observations.

The ordinary drift invocation uses the same resolved commit for its ancestry
comparison. Both modes require GNU `base64`; record mode also requires Python
3.11 or newer. Run the offline entry-point control from the checkout root:

```sh
env -i PATH=/usr/bin:/bin LANG=C.UTF-8 bash scripts/upstream_release_input_test.sh
```

To retain the synthetic repositories, inputs, outputs, and records, add
`RELEASE_TEST_EVIDENCE_DIR=/path/to/new-directory` after `env -i`. The
specified destination must not exist. Retain the command and its exit and
capture the test output separately.
