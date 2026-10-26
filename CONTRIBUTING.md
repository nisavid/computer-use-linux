# Contributing to computer-use-linux

Thanks for helping improve Linux desktop control for MCP hosts.

## Development Setup

```bash
git clone https://github.com/agent-sh/computer-use-linux.git
cd computer-use-linux
cargo check --locked
cargo test --locked
```

For npm wrapper work:

```bash
node --check npm/install.js
node --check npm/bin/computer-use-linux.js
node --test scripts/release_integrity_test.mjs
npm pack --dry-run
```

For Pi extension development:

```bash
node scripts/check_npm_lockfile_integrity.mjs pi/package-lock.json
node --test scripts/check_npm_lockfile_integrity_test.mjs
npm ci --prefix pi --ignore-scripts --no-audit --no-fund
npm run build:check --prefix pi
npm run typecheck --prefix pi
COMPUTER_USE_LINUX_TEST_BINARY="$PWD/target/debug/computer-use-linux" npm test --prefix pi
```

The Pi lock pins SHA-512 digests for every remote tarball. It deliberately omits
`hasShrinkwrap` on the Pi coding-agent entry: npm 11 otherwise reloads that
package's published shrinkwrap during `npm ci`, replacing the checked-in child
digests with incomplete upstream metadata. The complete locked dependency tree
and all versions stay unchanged. Run the dependency-free check before installing
after any lockfile regeneration; do not restore shrinkwrap reload flags. The npm
regression test checks both a valid install and rejection of substituted bytes.

## Before Opening a PR

Run the same gates CI runs:

```bash
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --no-fail-fast
scripts/install_sh_test.sh
scripts/mcp_safety_check.py
scripts/check_plugins.sh
agnix .
```

If you changed release packaging, also run:

```bash
cargo publish --dry-run --locked
npm pack --dry-run
```

## PR Guidelines

- Keep changes focused and explain the desktop/session you tested on.
- Include `computer-use-linux doctor` output for compositor, portal, or accessibility issues.
- Preserve the MCP safety annotations when adding or changing tools.
- Update `README.md`, `npm/README.md`, and `skills/computer-use-linux/SKILL.md` when user-facing commands change.
- After editing `skills/computer-use-linux/`, copy it into `plugins/computer-use-linux/skills/`. A release bumps the version in `Cargo.toml`, `package.json`, the plugin launcher, both plugin manifests, and `.claude-plugin/marketplace.json`. `scripts/check_plugins.sh` enforces both.
- Use conventional commit prefixes when practical (`fix:`, `feat:`, `docs:`, `chore:`).

## Release download integrity

The npm package and plugin ship version-bound SHA-256 pins for both binaries on
both architectures. Keep `npm/release-sha256.txt` and
`plugins/computer-use-linux/release-sha256.txt` identical. A version bump must
include reviewed pins from the corresponding trusted build outputs; missing or
stale pins fail closed. Run
`node scripts/update_release_pins.mjs /path/to/trusted-build-artifacts` with all
four named release binaries, then review and commit both manifests. Do not
generate pins from a mirror or its adjacent checksum files.

The initial 0.7.10 pins record GitHub's upstream release asset SHA-256 metadata
observed on 2026-10-26. This establishes a reviewed baseline against later
origin substitution; it is not an independent attestation of that original
upstream build. Future npm publication derives its pins from immutable artifacts
of the same build workflow. Plugin pins remain reviewed source files and must be
updated before distributing a new plugin version. The fork's release and
publication restrictions still apply.

Before uploading release assets, CI checks each architecture's build against
the committed pins. Publication also checks all downloaded workflow artifacts
against those pins. A rebuild that changes bytes must receive updated reviewed
pins before release; it cannot silently break source-distributed plugins.

The installer regressions use disposable package/plugin copies with fixture
digests and a private HTTPS server. They never override production pins through
environment variables. Automatic plugin downloads require curl because wget's
HTTPS-only setting does not restrict redirect protocols.

## Security

Do not open public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).
