#!/usr/bin/env node
// Exercise the shipped installers against an untrusted local release server.
// Fixture packages replace their bundled pins; production packages never do.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs/promises';
import { accessSync, constants } from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import os from 'node:os';
import path from 'node:path';
import { after, test } from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const pkg = JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
const args = process.argv.slice(2);
assert.ok(args.length === 0 || (args.length === 2 && args[0] === '--binary'),
  'usage: node scripts/release_integrity_test.mjs [--binary PATH]');
const realBinary = args.length ? path.resolve(args[1]) : undefined;
const arch = { x64: 'x86_64', arm64: 'aarch64' }[process.arch];
assert.equal(process.platform, 'linux', 'installer regressions require Linux');
assert.ok(arch, `unsupported test architecture: ${process.arch}`);
const target = `${arch}-unknown-linux-gnu`;
const names = ['computer-use-linux', 'computer-use-linux-cosmic'];
const assets = names.map(name => `${name}-${target}`);
const sha = value => createHash('sha256').update(value).digest('hex');
const mainBody = Buffer.from('#!/bin/sh\nprintf "helper=%s\\n" "$COMPUTER_USE_LINUX_COSMIC_HELPER"\nexec "$COMPUTER_USE_LINUX_COSMIC_HELPER" "$@"\n');
const helperBody = Buffer.from('#!/bin/sh\nprintf "verified-helper:%s\\n" "$*"\n');
const maliciousBody = Buffer.from('#!/bin/sh\nprintf malicious > "$EXECUTION_MARKER"\n');
const temporary = await fs.mkdtemp(path.join(os.tmpdir(), 'cul-release-integrity-'));
let nextId = 0;
const releases = new Map();
const httpRequests = [];

function executable(name) {
  for (const dir of (process.env.PATH || '').split(path.delimiter)) {
    const candidate = path.join(dir, name);
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch { /* Try the next PATH entry. */ }
  }
  return undefined;
}

function openssl(...parameters) {
  const result = spawnSync('openssl', parameters, { encoding: 'utf8' });
  assert.equal(result.status, 0, `openssl ${parameters[0]} failed: ${result.stderr || result.error}`);
}

const ca = path.join(temporary, 'ca.crt');
const caKey = path.join(temporary, 'ca.key');
const key = path.join(temporary, 'localhost.key');
const csr = path.join(temporary, 'localhost.csr');
const cert = path.join(temporary, 'localhost.crt');
const extensions = path.join(temporary, 'localhost.ext');
openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
  '-subj', '/CN=computer-use-linux installer test CA', '-addext', 'basicConstraints=critical,CA:TRUE',
  '-keyout', caKey, '-out', ca);
openssl('req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=localhost', '-keyout', key, '-out', csr);
await fs.writeFile(extensions, 'subjectAltName=DNS:localhost\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n');
openssl('x509', '-req', '-in', csr, '-CA', ca, '-CAkey', caKey, '-CAcreateserial',
  '-days', '1', '-extfile', extensions, '-out', cert);

const httpTrap = http.createServer((request, response) => {
  httpRequests.push(request.url);
  response.writeHead(200);
  response.end(maliciousBody);
});
await new Promise(resolve => httpTrap.listen(0, resolve));
const httpBase = `http://localhost:${httpTrap.address().port}`;
const server = https.createServer({ key: await fs.readFile(key), cert: await fs.readFile(cert) }, (request, response) => {
  const [, id, endpoint, asset] = request.url.split('/');
  const release = releases.get(id);
  if (!release || !asset) {
    response.writeHead(404).end();
    return;
  }
  release.requests.push(request.url);
  if (release.redirect && endpoint === 'release') {
    const origin = release.redirect === 'https' ? httpsBase : httpBase;
    const location = release.redirect === 'relative' ? `../mirror/${asset}` : `${origin}/${id}/mirror/${asset}`;
    response.writeHead(302, { location }).end();
    return;
  }
  if (release.offline) {
    response.writeHead(503).end();
    return;
  }
  const basename = asset.replace(/\.sha256$/, '');
  const body = release.bodies.get(basename);
  if (!body) {
    response.writeHead(404).end();
    return;
  }
  response.writeHead(200);
  response.end(asset.endsWith('.sha256') ? `${sha(body)}  ${basename}\n` : body);
});
await new Promise(resolve => server.listen(0, resolve));
const httpsBase = `https://localhost:${server.address().port}`;
after(async () => {
  await Promise.all([server, httpTrap].map(instance => new Promise(resolve => instance.close(resolve))));
  await fs.rm(temporary, { recursive: true, force: true });
});

async function fixture(kind, { tool = 'curl', bodies = [mainBody, helperBody] } = {}) {
  const id = String(++nextId);
  const directory = path.join(temporary, id);
  await fs.mkdir(directory);
  await fs.copyFile(path.join(root, 'package.json'), path.join(directory, 'package.json'));
  const source = kind === 'npm' ? 'npm' : 'plugins/computer-use-linux';
  const packageDir = path.join(directory, source);
  await fs.cp(path.join(root, source), packageDir, {
    recursive: true,
    filter: file => kind !== 'npm' || path.dirname(file) !== path.join(root, 'npm/bin') || file.endsWith('.js'),
  });
  const pins = path.join(packageDir, 'release-sha256.txt');
  // Include both supported architectures, as real packages do.
  const manifest = ['x86_64', 'aarch64'].flatMap(cpu => names.map((name, index) =>
    `${pkg.version} ${sha(bodies[index])} ${name}-${cpu}-unknown-linux-gnu`)).join('\n') + '\n';
  await fs.writeFile(pins, manifest);
  const tools = path.join(directory, 'tools');
  await fs.mkdir(tools);
  for (const name of ['basename', 'cat', 'chmod', 'cut', 'dirname', 'find', 'grep', 'head',
    'mkdir', 'mktemp', 'mv', 'rm', 'sed', 'sha256sum', 'shasum', 'touch', 'tr', 'uname', tool]) {
    const command = executable(name);
    if (command) await fs.symlink(command, path.join(tools, name));
  }
  assert.ok(executable(tool), `${tool} is required for installer regression coverage`);
  const environment = { ...process.env };
  for (const name of Object.keys(environment)) {
    if (name.startsWith('COMPUTER_USE_LINUX_') || /^(https?|all)_proxy$/i.test(name) || name === 'NODE_TLS_REJECT_UNAUTHORIZED') {
      delete environment[name];
    }
  }
  const cacheDir = path.join(directory, 'cache/computer-use-linux/plugin', `verified-v${pkg.version}`);
  Object.assign(environment, {
    HOME: directory, XDG_CACHE_HOME: path.join(directory, 'cache'), PATH: tools,
    COMPUTER_USE_LINUX_DOWNLOAD_BASE: `${httpsBase}/${id}/release`,
    NODE_EXTRA_CA_CERTS: ca, CURL_CA_BUNDLE: ca, SSL_CERT_FILE: ca,
    EXECUTION_MARKER: path.join(directory, 'executed'),
  });
  const release = {
    kind, id, directory, packageDir, pins, environment, cacheDir, requests: [],
    bodies: new Map(assets.map((asset, index) => [asset, bodies[index]])),
    launcher: path.join(packageDir, 'bin/computer-use-linux'),
    installed: kind === 'npm'
      ? [path.join(packageDir, `bin/computer-use-linux-${process.platform}-${process.arch}`), path.join(packageDir, 'bin/computer-use-linux-cosmic')]
      : names.map(name => path.join(cacheDir, name)),
  };
  releases.set(id, release);
  return release;
}

function run(command, parameters, environment, timeout = 20_000) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, parameters, { env: environment, cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', chunk => { stdout += chunk; });
    child.stderr.on('data', chunk => { stderr += chunk; });
    const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`${command} timed out: ${stderr}`)); }, timeout);
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.on('close', (status, signal) => { clearTimeout(timer); resolve({ status, signal, stdout, stderr }); });
  });
}

function install(release, extraEnvironment = {}) {
  const environment = { ...release.environment, ...extraEnvironment };
  return release.kind === 'npm'
    ? run(process.execPath, [path.join(release.packageDir, 'install.js')], environment)
    : run(release.launcher, ['probe'], environment);
}

function succeeded(result) {
  assert.equal(result.status, 0, `subprocess failed: ${result.stderr}\n${result.stdout}`);
}

async function rejected(release, pattern) {
  const result = await install(release);
  assert.notEqual(result.status, 0, `installer unexpectedly succeeded: ${result.stdout}`);
  assert.match(result.stderr, pattern);
  assert.equal(await exists(release.environment.EXECUTION_MARKER), false, 'untrusted bytes executed');
  return result;
}

async function exists(file) {
  try { await fs.access(file); return true; } catch { return false; }
}

function noRemoteChecksums(release) {
  assert.equal(release.requests.some(request => request.endsWith('.sha256')), false,
    'installer requested a checksum from the untrusted release origin');
}

test('shipped trust manifests match and pin both assets for both architectures', async () => {
  const npm = await fs.readFile(path.join(root, 'npm/release-sha256.txt'), 'utf8');
  assert.equal(npm, await fs.readFile(path.join(root, 'plugins/computer-use-linux/release-sha256.txt'), 'utf8'));
  const records = npm.trim().split('\n');
  assert.equal(records.length, 4);
  const keys = records.map(record => {
    assert.match(record, /^\S+ [a-f0-9]{64} computer-use-linux(?:-cosmic)?-(?:x86_64|aarch64)-unknown-linux-gnu$/);
    const [version, , asset] = record.split(' ');
    assert.equal(version, pkg.version);
    return asset;
  });
  assert.equal(new Set(keys).size, 4);
});

test('pin generation hashes all four trusted local artifacts and mirrors the manifest', async () => {
  const release = await fixture('npm');
  const scripts = path.join(release.directory, 'scripts');
  const plugin = path.join(release.directory, 'plugins/computer-use-linux');
  const artifacts = path.join(release.directory, 'trusted-artifacts');
  await Promise.all([scripts, plugin, artifacts].map(dir => fs.mkdir(dir, { recursive: true })));
  await fs.copyFile(path.join(root, 'scripts/update_release_pins.mjs'), path.join(scripts, 'update_release_pins.mjs'));
  const records = ['aarch64', 'x86_64'].flatMap(cpu => names.map(name => `${name}-${cpu}-unknown-linux-gnu`)).sort();
  for (const asset of records) await fs.writeFile(path.join(artifacts, asset), `trusted-local-build:${asset}\n`);
  const generator = [path.join(scripts, 'update_release_pins.mjs'), artifacts];
  succeeded(await run(process.execPath, generator, release.environment));
  const expected = records.map(asset => `${pkg.version} ${sha(`trusted-local-build:${asset}\n`)} ${asset}\n`).join('');
  assert.equal(await fs.readFile(release.pins, 'utf8'), expected);
  const pluginPins = path.join(plugin, 'release-sha256.txt');
  assert.equal(await fs.readFile(pluginPins, 'utf8'), expected);

  const checker = [generator[0], '--check', artifacts];
  succeeded(await run(process.execPath, checker, release.environment));
  succeeded(await run(process.execPath, [...checker, 'aarch64-unknown-linux-gnu'], release.environment));
  await fs.writeFile(path.join(artifacts, records[0]), 'different rebuild bytes');
  const changed = await run(process.execPath, checker, release.environment);
  assert.notEqual(changed.status, 0);
  assert.match(changed.stderr, /build artifact differs from committed release pin/);
  assert.equal(await fs.readFile(release.pins, 'utf8'), expected);
  assert.equal(await fs.readFile(pluginPins, 'utf8'), expected);
  await fs.writeFile(path.join(artifacts, records[0]), `trusted-local-build:${records[0]}\n`);
  await fs.writeFile(pluginPins, 'stale plugin pins');
  const inconsistent = await run(process.execPath, checker, release.environment);
  assert.notEqual(inconsistent.status, 0);
  assert.match(inconsistent.stderr, /npm and plugin release pins differ/);
  await fs.writeFile(pluginPins, expected);

  // A missing artifact must leave both trust copies intact.
  await fs.unlink(path.join(artifacts, records.at(-1)));
  // A matrix job can validate its own complete architecture before uploading.
  succeeded(await run(process.execPath, [...checker, 'aarch64-unknown-linux-gnu'], release.environment));
  const incomplete = await run(process.execPath, checker, release.environment);
  assert.notEqual(incomplete.status, 0);
  assert.match(incomplete.stderr, /ENOENT/);
  const missing = await run(process.execPath, generator, release.environment);
  assert.notEqual(missing.status, 0);
  assert.match(missing.stderr, /ENOENT/);
  assert.equal(await fs.readFile(release.pins, 'utf8'), expected);
  assert.equal(await fs.readFile(pluginPins, 'utf8'), expected);
  const noInput = await run(process.execPath, generator.slice(0, 1), release.environment);
  assert.notEqual(noInput.status, 0);
  assert.match(noInput.stderr, /usage:/);
  assert.equal(release.requests.length, 0);
});

for (const kind of ['npm', 'plugin']) {
  test(`${kind}: valid HTTPS install preserves helper handoff without fetching remote checksums`, async () => {
    const release = await fixture(kind);
    succeeded(await install(release));
    for (let index = 0; index < assets.length; index++) {
      assert.deepEqual(await fs.readFile(release.installed[index]), release.bodies.get(assets[index]));
    }
    const result = kind === 'npm'
      ? await run(process.execPath, [path.join(release.packageDir, 'bin/computer-use-linux.js'), 'probe'], release.environment)
      : await install(release);
    succeeded(result);
    assert.equal(result.stdout, `helper=${release.installed[1]}\nverified-helper:probe\n`);
    assert.equal(release.requests.length, 2);
    noRemoteChecksums(release);
  });

  for (const [index, name] of names.entries()) {
    test(`${kind}: ${name} substitution with a matching remote checksum is rejected`, async () => {
      const release = await fixture(kind);
      // This origin serves an adjacent checksum matching the malicious bytes.
      release.bodies.set(assets[index], maliciousBody);
      await rejected(release, /sha256 mismatch/i);
      assert.equal(await exists(release.installed[0]), false, 'failed download reached the install directory');
      noRemoteChecksums(release);
    });
  }

  for (const invalid of ['missing file', 'wrong version', 'missing helper', 'missing inactive pin', 'malformed record',
    'noncanonical spacing', 'unknown asset', 'duplicate record', 'inactive duplicate record']) {
    test(`${kind}: ${invalid} trusted pins fail before network access`, async () => {
      const release = await fixture(kind);
      const manifest = await fs.readFile(release.pins, 'utf8');
      if (invalid === 'missing file') await fs.unlink(release.pins);
      if (invalid === 'wrong version') await fs.writeFile(release.pins, manifest.replaceAll(pkg.version, '0.0.0'));
      if (invalid === 'missing helper') await fs.writeFile(release.pins, manifest.split('\n').filter(line => !line.includes('-cosmic-')).join('\n'));
      if (invalid === 'missing inactive pin') {
        const inactive = manifest.split('\n').find(line => line && !line.endsWith(`-${target}`));
        await fs.writeFile(release.pins, manifest.split('\n').filter(line => line !== inactive).join('\n'));
      }
      if (invalid === 'malformed record') await fs.writeFile(release.pins, manifest.replace(/ [a-f0-9]{64} /, ' invalid '));
      if (invalid === 'noncanonical spacing') await fs.writeFile(release.pins, manifest.replace(' ', '  '));
      if (invalid === 'unknown asset') await fs.writeFile(release.pins, manifest + `${pkg.version} ${sha(mainBody)} unexpected-asset\n`);
      if (invalid === 'duplicate record') await fs.writeFile(release.pins, manifest + manifest.split('\n')[0] + '\n');
      if (invalid === 'inactive duplicate record') await fs.writeFile(release.pins, manifest + manifest.split('\n').find(line => line && !line.endsWith(`-${target}`)) + '\n');
      await rejected(release, /pin|manifest|sha256|checksum|ENOENT/i);
      assert.equal(release.requests.length, 0);
    });
  }

  test(`${kind}: a trusted manifest without a final newline is accepted`, async () => {
    const release = await fixture(kind);
    await fs.writeFile(release.pins, (await fs.readFile(release.pins, 'utf8')).trimEnd());
    succeeded(await install(release));
    assert.equal(release.requests.length, 2);
    noRemoteChecksums(release);
  });

  test(`${kind}: HTTPS URL schemes are case-insensitive`, async () => {
    const release = await fixture(kind);
    release.environment.COMPUTER_USE_LINUX_DOWNLOAD_BASE = release.environment.COMPUTER_USE_LINUX_DOWNLOAD_BASE.replace('https:', 'HTTPS:');
    succeeded(await install(release));
    assert.equal(release.requests.length, 2);
    noRemoteChecksums(release);
  });

  for (const redirect of ['https', 'relative']) {
    test(`${kind}: an HTTPS mirror with an ${redirect === 'https' ? 'absolute' : 'relative'} HTTPS redirect is allowed`, async () => {
      const release = await fixture(kind);
      release.redirect = redirect;
      succeeded(await install(release));
      assert.equal(release.requests.length, 4);
      noRemoteChecksums(release);
    });
  }

  test(`${kind}: HTTP initial URLs fail without making an HTTP request`, async () => {
    const release = await fixture(kind);
    release.environment.COMPUTER_USE_LINUX_DOWNLOAD_BASE = `${httpBase}/${release.id}/release`;
    const before = httpRequests.length;
    await rejected(release, /https|protocol/i);
    assert.equal(httpRequests.length, before);
    assert.equal(release.requests.length, 0);
  });

  test(`${kind}: HTTPS to HTTP downgrade redirects fail without making an HTTP request`, async () => {
    const release = await fixture(kind);
    release.redirect = 'http';
    const before = httpRequests.length;
    await rejected(release, /https|protocol/i);
    assert.equal(httpRequests.length, before);
    assert.ok(release.requests.length > 0, 'redirect endpoint was not reached');
  });

  test(`${kind}: explicit local binary overrides remain available without trusted pins or network`, async () => {
    const release = await fixture(kind);
    await fs.unlink(release.pins);
    const local = path.join(release.directory, 'local');
    await fs.writeFile(local, '#!/bin/sh\nprintf "local-override:%s\\n" "$*"\n', { mode: 0o755 });
    if (kind === 'npm') {
      const helper = path.join(release.directory, 'local-helper');
      await fs.writeFile(helper, helperBody, { mode: 0o755 });
      succeeded(await install(release, {
        COMPUTER_USE_LINUX_LOCAL_BINARY: local, COMPUTER_USE_LINUX_LOCAL_COSMIC_HELPER: helper,
      }));
      assert.deepEqual(await fs.readFile(release.installed[0]), await fs.readFile(local));
      assert.deepEqual(await fs.readFile(release.installed[1]), helperBody);
    } else {
      const result = await install(release, { COMPUTER_USE_LINUX_BIN: local });
      succeeded(result);
      assert.equal(result.stdout, 'local-override:probe\n');
    }
    assert.equal(release.requests.length, 0);
  });
}

test('npm: explicit skip-download route works without pins or network', async () => {
  const release = await fixture('npm');
  await fs.unlink(release.pins);
  succeeded(await install(release, { COMPUTER_USE_LINUX_SKIP_DOWNLOAD: '1' }));
  assert.equal(await exists(release.installed[0]), false);
  assert.equal(await exists(release.installed[1]), false);
  assert.equal(release.requests.length, 0);
});

test('plugin: cached binaries are reused without network access or curl', async () => {
  const release = await fixture('plugin');
  succeeded(await install(release));
  release.offline = true;
  await fs.unlink(path.join(release.environment.PATH, 'curl'));
  succeeded(await install(release));
  assert.equal(release.requests.length, 2);
});

for (const [index, name] of names.entries()) {
  test(`plugin: cached ${name} tampering is rejected before execution or network access`, async () => {
    const release = await fixture('plugin');
    succeeded(await install(release));
    await fs.writeFile(release.installed[index], maliciousBody);
    release.offline = true;
    await rejected(release, /sha256 mismatch/i);
    assert.equal(release.requests.length, 2);
  });
}

test('plugin: legacy unverified caches cannot supply or replace verified files', async () => {
  const release = await fixture('plugin');
  const legacy = path.join(path.dirname(release.cacheDir), `v${pkg.version}`);
  await fs.mkdir(legacy, { recursive: true });
  for (const name of names) await fs.writeFile(path.join(legacy, name), maliciousBody, { mode: 0o755 });
  succeeded(await install(release));
  assert.equal(release.requests.length, 2);
  assert.equal(await exists(release.environment.EXECUTION_MARKER), false);
  // Model a legacy install finishing after the new download. Its destination
  // is disjoint from the verified cache, even while hosts update independently.
  for (const name of names) await fs.writeFile(path.join(legacy, name), maliciousBody);
  release.offline = true;
  succeeded(await install(release));
  assert.equal(release.requests.length, 2);
  assert.equal(await exists(release.environment.EXECUTION_MARKER), false);
});

test('plugin: wget without curl fails clearly before any network request', async () => {
  const release = await fixture('plugin', { tool: 'wget' });
  await rejected(release, /curl is required.*https/i);
  assert.equal(release.requests.length, 0);
});

test('plugin: real release MCP smoke, cache reuse, and pruning', { skip: !realBinary }, async () => {
  const helper = path.join(path.dirname(realBinary), 'computer-use-linux-cosmic');
  const release = await fixture('plugin', { bodies: await Promise.all([realBinary, helper].map(file => fs.readFile(file))) });
  const python = executable('python3');
  assert.ok(python, 'python3 is required for the MCP safety check');
  // The smoke script itself needs a normal PATH; it constrains PATH in its own
  // desktop-isolation subtest, while this release remains confined to localhost.
  const environment = { ...release.environment, PATH: process.env.PATH };
  const smoke = () => run(python, [path.join(root, 'scripts/mcp_safety_check.py'), '--repo', root, '--binary', release.launcher], environment, 60_000);
  succeeded(await smoke());
  assert.equal(release.requests.length, 2);
  release.offline = true;
  succeeded(await smoke());
  assert.equal(release.requests.length, 2);
  const cacheRoot = path.dirname(release.cacheDir);
  const stale = path.join(cacheRoot, 'v0.0.0');
  const recent = path.join(cacheRoot, 'v0.0.1');
  const staging = path.join(cacheRoot, '.download.killed');
  await Promise.all([stale, recent, staging].map(dir => fs.mkdir(dir)));
  const now = Date.now();
  await fs.utimes(stale, new Date(now - 40 * 86_400_000), new Date(now - 40 * 86_400_000));
  await fs.utimes(staging, new Date(now - 2 * 3_600_000), new Date(now - 2 * 3_600_000));
  succeeded(await smoke());
  assert.equal(await exists(stale), false, 'launcher kept a version unused for 40 days');
  assert.equal(await exists(recent), true, 'launcher pruned a recently used version');
  assert.equal(await exists(staging), false, 'launcher kept stale staging files');
  assert.equal(release.requests.length, 2);
});
