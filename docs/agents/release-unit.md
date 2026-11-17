# Release-unit interfaces

Use this reference with `release-preparation.md` when a builder, publisher, or
qualification consumer exchanges a release unit. These v1 interfaces bind the
selected KDE Wayland x86_64 profile. Builders, validators, and publishers must
enforce the relevant fields and observations before effects.

## Records and validation

Retain UTF-8 JSON bytes and their external lowercase SHA-256 digests. Use
schema identifiers `computer-use-linux/incorporated-baseline/v1`,
`computer-use-linux/release-manifest/v1`, and
`computer-use-linux/release-receipt/v1` respectively. Reject missing fields,
wrong types, unsupported schemas, unresolved identities, duplicate JSON keys,
duplicate asset names or normalized inventory paths, unsafe paths, and
contradictory bindings before an effect. Retain full immutable Git object IDs;
never substitute an unverified branch name. Cross-check every version, source,
runtime digest, asset, destination, and prerequisite receipt.

The known-baseline verifier produces:

- `upstream_repository`, `version`, and `release`: positive numeric `id`,
  stable `tag`, `draft: false`, `prerelease: false`, `published_at`, and
  `body_sha256`.
- `source`: `tag_ref`, `tag_object`, `tag_object_type`, peeled `commit`,
  `cargo_version`, and `cargo_sha256`.
- `verification`: `verifier_revision`, observation/evidence references and
  SHA-256 digests, commands and statuses, and the matching first/second
  release/ref observations.
- `incorporation`: `candidate_commit`, accepted `integration_commit`,
  ancestry evidence, `divergence_record_sha256`, and separately identified
  selected later upstream patches. Missing stable release/tag verification
  yields a refusal, never a completed baseline record.

The release manifest contains these required fields:

- `schema`, `repository: "nisavid/computer-use-linux"`,
  `procedure_revision`, `candidate_commit`, and
  `divergence_record_sha256`.
- `incorporated_baseline_sha256`; `latest_observation_sha256` is a digest or
  explicit null and has comparison-only meaning.
- `allocation`: `numeric_version`, `full_version`,
  `inventory_sha256`, `allocation_receipt_sha256`, and per-route effective
  identity rules/results. A synthetic manifest uses an explicit
  `synthetic: true` and null allocation receipt, and is ineligible for publishing.
  A real manifest uses `synthetic: false` and a verified allocation receipt.
- `build`: `builder_revision`, `toolchain`, `locked_inputs` with immutable
  identities/digests, `runtime_inputs`, and reproduction inputs.
- `profile`: `architecture: "x86_64"`, `session: "wayland"`,
  `compositor: "KDE KWin"`, and supported claims/limits.
- `runtime`: binary name, actual reported full version, size, SHA-256, and
  immutable fork release-asset repository/tag/name/download identity.
- `assets`: complete expected published asset list with unique names, roles,
  sizes, and SHA-256 digests. Roles cover the runtime, account ZIP, catalog
  package, and any selected checksum artifact. The manifest itself is bound
  by its external digest and is excluded from its own hashed asset list.
- `packages`: `account_zip` and `catalog`, each with package SHA-256,
  full version, launcher SHA-256, shared runtime SHA-256, bundled pin SHA-256,
  and complete inventory entries (`path`, `type`, `mode`, `size`,
  `sha256`). For directories, size and digest are explicit null. Declare archive
  roots, ordering, timestamps, permissions, and allowed entry types as construction
  inputs; define any link target and its treatment explicitly and reject unlisted
  entries and escaping paths/links. Inventory modes distinguish executable launchers.
- `catalog`: repository, immutable commit, catalog path/tree identity, and
  `stable_ref: "refs/heads/latest-stable"`.
- `publication`: `tag_ref: "refs/tags/v<full_version>"` expanded with the
  allocated full version, expected source commit, exact
  tag object/type when it exists, expected release ID or explicit null before
  creation, and destination repository. Bind creation results and each
  operation's observed preimages in receipts; do not rewrite the manifest
  after exposure. Promotion retains its observed stable-ref preimage or
  explicit null when absent in the operation record.

Keep the manifest outside the archives/catalog content it hashes; no object
contains its own digest or its containing Git commit's ID. Bind the final
catalog commit in the external manifest after construction. Exposed manifest
changes require reconciliation under the same changed-content rule as other
exposed bytes.

A receipt contains `schema`, `kind`, `status` (pass, fail, refused, or
unknown), `manifest_sha256`, `procedure_revision`, `invocation`,
`observed_at`, `evidence` references/digests, `prerequisite_receipts`,
`identities`, `observations`, `limitations`, and `next_action`.
An allocation receipt precedes the manifest: it instead binds candidate,
baseline/inventory digests, numeric/full version, destinations, and reservation
result; its `manifest_sha256` is null. It gains no retroactive authority from
a later manifest. Evidence location and access controls must let the relevant
consumer verify it. Keep private account identifiers and observations in
restricted receipts, outside public manifests/packages; include no credentials.

## Required receipt observations

| Kind | Required observations |
| --- | --- |
| allocation | Complete inventory and preimage, chosen identity/reason, serialization, recheck, reservation or allocation result, unchanged-retry binding. |
| construction | Source/build identities, equivalent frozen-input reproduction, runtime reported version and digest, both package inventories/permissions, independently bundled pins, review and validation evidence. |
| native | Runtime digest, claimed behavior matrix, actual compositor/portal/toolkit/layout/targets/backend, isolated target/decoy and independent effect observations, retained failures and cleanup. |
| draft | Release ID and draft state, exact tag/source and complete asset provider identities/digests, authenticated retrieval, construction prerequisite. |
| route | Route/client and stored full version, package/materialized inventory, launcher/downloaded runtime digest, discovery, actual execution, changed-version update and prior version, draft overrides/limits, immutable remote transport where applicable. |
| publication | Existing release ID, public state, unchanged tag/assets/manifest, GitHub latest disabled, draft/native/route prerequisites and observed result. |
| public-download | Ordinary launcher path with fresh caches and no override, anonymous public asset retrieval/digest, executed runtime, both routes' relevant results. |
| promotion | Required passing receipts, observed stable-ref preimage, intended immutable catalog commit, guarded result and final ref/content readback. |
| recovery | Original operation and uncertain/partial result, observed provider/ref/account identities, intended-versus-observed classification, remaining missing steps, current authority. |

Keep construction, reproduction, review, and qualification receipt digests outside
this immutable manifest. Receipts reference the manifest, so the manifest cannot
also hash those receipts. Allocation is the pre-manifest exception defined above.

A stage passes only when all required observations exist for the same frozen
release unit and relevant dependencies. A changed source, asset, input, or
procedure invalidates affected evidence. A constructed service, local copied
client state, authenticated draft download, and anonymous remote install each
establish their observed scope; none substitutes for another.
