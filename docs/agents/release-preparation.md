# Prepare and deliver a stable fork release

Use this procedure to propose or reconcile a version, construct a release unit,
stage or qualify its assets, publish it, promote its catalog, or recover a
partial delivery. The [release contract](https://github.com/nisavid/computer-use-linux/issues/44)
and [amended allocation decision](https://github.com/nisavid/real-skills/issues/108#issuecomment-6932838799)
govern this procedure. Load the [release-unit interfaces](release-unit.md)
before constructing or consuming a manifest or receipt.

Each operation needs its current authority and evidence. Procedure development
and synthetic exercises allocate nothing. Preserve prior preparation and
source-resolution results as historical evidence.

## Establish the inputs

1. Record the procedure revision, active scope and holds, intended operation,
   fork repository, and immutable candidate revision. Read fork stewardship and
   the divergence inventory. Recover the original authority before a dependent
   effect; a handoff or a successful earlier stage does not release a hold.
2. Keep latest discovery separate from incorporation. A retained
   `computer-use-linux/upstream-release-input/v1` record from
   `docs/testing/upstream-release-input.md` is an observation of latest stable
   upstream when it was obtained. Verify its digest and recorded identities
   before using it for comparison. Refreshing latest needs a new record; it
   does not change the candidate's baseline.
3. Establish a separately verified incorporated-baseline record. Bind the
   upstream repository, stable version and release ID, tag ref/object/type,
   peeled commit, Cargo version/blob digest, release notes digest, observation
   evidence, and verifier revision. Bind incorporation to the candidate using
   preserved upstream ancestry and the accepted integration/divergence record.
   List selected later patches separately. Source ancestry and a Cargo version
   alone do not prove the upstream release/tag identity.

   For a known historical baseline, use the bounded manual path below. A
   generic source selector, new Fork Ops equipment, and a broad upstream merge
   are not prerequisites. If the baseline or incorporation cannot be verified,
   return a refusal naming the missing evidence; derive no usable version.
4. Freeze the integrated fork candidate and carried divergence by revision and
   digest. Record current checks/review, the builder revision, locked toolchain
   and runtime inputs, supported profile, and qualification still needed.
   Compare newer upstream only for a named behavior, dependency, or
   compatibility need; keep that decision separate from latest discovery.
5. Assemble the complete allocation, reservation, and exposure inventory for
   the incorporated major/minor line. Include public and private releases,
   drafts, tags, package stores and account releases, failed/partial uploads,
   withdrawn/deleted identities, and standalone reservations. For each record,
   bind full and numeric versions, owner/candidate, upstream record, destination,
   route-effective identity, exposed artifacts and their retained digests, and
   current/withdrawn/unknown state. Identify the authoritative sources, access
   scope, observation times, and evidence establishing completeness. An absent
   list, public listing, or current-only account view is not a complete inventory.
   Account access and other reads retain their own authority.

Completion of this stage means that required identities and inventory are
known, or that a precise refusal has been retained. No proposed version is
usable while a necessary input or ownership remains unknown.

### Verify a known incorporated baseline manually

In an authorized isolated checkout, verify repository and read-only upstream
identity first. Select the already identified historical tag explicitly; keep
the existing latest-stable resolver unchanged.

1. Read the upstream release by that tag and its exact tag ref. Validate the
   repository, positive release ID, stable tag, non-draft/non-prerelease state,
   publication time, and notes. Retain the response and observation evidence.
2. Fetch only the selected tag from the verified upstream into an isolated
   fetch destination. Compare the fetched object and type with the provider
   ref, peel it to a commit, and read that commit's Cargo blob. The stable
   version and Cargo version must agree.
3. Read the same release and tag ref again. Require unchanged release identity,
   tag object, stable state, publication time, and notes. Retain original
   commands, exit status, diagnostics, both observations, object identities,
   Cargo and notes SHA-256 digests, verifier revision, and the record's digest.
   Concurrent observations are not an atomic provider snapshot.
4. Verify that this commit is incorporated into the frozen candidate using
   Git with replacement objects disabled and the accepted integration record.
   Account separately for carried fork changes and selected later patches.

Use these same checks when a retained historical record already supplies the
inputs; retain which observations were reused and why they remain applicable.
Do not relabel a latest-observation record as a historical-baseline record.
Changed provider identities, incomplete Git history, missing objects, or
unexplained integration require reconciliation before preparation.

## Propose or reconcile the version

Let the incorporated stable version be `M.m.U`. Preserve its major/minor line.

1. Reconcile every existing binding to the candidate/source, independent of
   inventory order. Establish the intended full version from evidence rather
   than choosing the newest entry. Unknown intent or contradictory ownership
   requires reconciliation.
2. For that version, classify every intended route at its effective identity.
   An exposed artifact must match its own retained digest. A candidate-owned
   unexposed reservation must match candidate, baseline, and full version. A
   free route needs affirmative evidence of no allocation, reservation, or
   exposure. A differently bound occupied identity is a collision even if the
   bytes match. Unknown bytes or ownership require refusal.
3. Preserve the intended full version for an unchanged retry only when all
   exposed route artifacts match and all remaining routes are verified free
   or correctly bound. Different route artifacts can have different digests.
   Preserve matching standalone reservations. Drafts and failed account
   updates retain any reservation or exposure they created.
4. For a fresh identity, enumerate numeric patches in increasing order, at
   least `U` and advancing the fork's exposed numeric sequence in this line.
   Include private and withdrawn exposure in that sequence. Choose the first
   number whose numeric identity and every intended route-effective identity
   are available after reconciling allocations and reservations. With highest
   exposed patch `H`, enumeration starts at `max(U, H + 1)`; with no exposure,
   it starts at `U`. Occupied reservations can exclude a number without
   pretending they were exposed. A later upstream patch does not reset history.
5. Use plain `M.m.U` only when the selected number equals `U` and affirmative
   evidence establishes equality eligibility: sequence advancement,
   availability/ownership at all intended identities, and applicable prior
   exposure comparisons. Modified fork source alone does not disqualify
   equality. A verified change to exposed bytes requires a new higher numeric
   identity; metadata alone cannot supply that increment. Otherwise use
   `M.m.P+from.M.m.U`. Recheck all bindings before returning a proposal.

Record why this is an unchanged retry, eligible equality, or a new numeric
identity; bind the baseline record, candidate, inventory, chosen number,
skipped collisions, full version, and route identity rules. Both packages use
that same full version. Unsupported storage or version normalization requires
a demonstrated consumer result and a decision before exposure.

A proposal creates no reservation. If applying the proposed version changes
source, freeze that new revision and renew affected checks/review and bindings
before allocation. Actual allocation is a separately authorized, serialized
operation that reobserves the inventory and retains its receipt.
Changed exposed content keeps the old exposure and starts a new identity;
changed unexposed content needs renewed construction/review evidence.

## Construct and qualify one release unit

1. Consume the reviewed manifest interface. Build one main x86_64 runtime for
   the selected KDE Wayland profile from reviewed fork source. Record locked
   inputs and reproducibility evidence. Both installation packages download
   the same immutable fork asset and bundle its independently trusted checksum.
   Verify bytes before execution and cache reuse; refuse missing pins, wrong
   versions/bytes, and unavailable downloads without an unverified fallback.
2. Construct the account-importable ZIP and immutable catalog package. Preserve
   product names and non-selected registry publishers. Bind inventories,
   executable permissions, full version, launcher, exact download identity,
   runtime digest, and catalog commit. Additional architectures and COSMIC
   helper artifacts are outside this profile.
3. Validate the manifest, constructed contents, package/version consistency,
   reproducibility, refusal/cache behavior, and contained starts. Bind reviews
   and native evidence to the actual source and runtime digest. Method tests,
   a successful API response, or prior-candidate observations retain their scope.

Construction and qualification consume the interfaces; neither operation
allocates or publishes by itself.

## Stage, publish, and promote

1. Under current authority, the repository Actions publisher validates repository,
   reviewed source, full version/tag, exact assets, and release state. Use
   `refs/tags/v<full_version>` for the versioned tag. Serialize
   competing writes. Create or reconcile a draft and upload only missing
   approved assets. Observe each result. Preserve differing exposed assets and
   reconcile them; never replace them as an upload retry. Keep workflow
   permissions read-only by default and confine contents-write to publishing
   jobs. Preserve protections and upstream crate/npm publisher guards.
2. Retrieve the draft assets with repository access, verify the manifest and
   digests, and qualify those bytes. Record every draft-only download/runtime
   override and its limits. Keep credentials out of packages and client copies.
   Use supported attended account import/update until an unattended API has
   been demonstrated. Verify stored full versions, materialized contents, tool
   discovery, actual execution, and changed-version update on both account ZIP
   and immutable remote-catalog routes.
3. After required draft/native/route evidence passes, publish the existing draft
   and unchanged assets with GitHub latest selection disabled. Reobserve the
   release and complete inventory. Run ordinary public downloads using the
   unchanged launchers, fresh caches, and no draft-only overrides. A draft
   receipt does not prove anonymous public download behavior.
4. Only after required receipts pass, compare the observed preimage of
   `refs/heads/latest-stable` with the authorized preimage and move it to the
   verified immutable catalog commit. Handle an absent ref explicitly.
   Reobserve the ref and catalog contents. A competing writer, failed public
   download, or incomplete receipt withholds promotion.

Retain state transitions and their receipts separately. Native qualification,
installation/update, publication, catalog promotion, and a Linear release's
Released state have distinct completion criteria. The first qualified release
and the automated synchronization/release lifecycle are separate milestones.

## Recover and hand back

On interruption or uncertain write outcome, read the release ID/state, tag
object/commit, assets by provider identity and digest, account release/current
association, reservations, and stable ref before another write. Compare with
the intended manifest and retained preimages. Resume only verified missing
steps with current authority. A failed account update can leave a non-current
release that consumed the version; record that exposure separately from repair
of a fixture or proof of the product route.

Return the consumed procedure revision, input digests, inventory/completeness,
proposal or allocation receipt, release-unit manifest digest, stage receipts,
limitations, and next necessary evidence, or a precise refusal. Unknown outcomes
remain unknown until readback resolves them.

Before procedure completion, independently review the final committed revision
and invocation chain, exercise discovery separately from application, and have
a consumer load that reviewed revision. Retain a useful application or truthful
refusal showing that the amended rule controls. After authorized installation,
read back affected entry points and downstream instructions; report any
unreconciled projection or adoption without claiming completion. A naturally
useful consumer use is sufficient; do not perform a real allocation merely to
test the procedure.
