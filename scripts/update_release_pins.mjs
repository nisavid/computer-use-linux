#!/usr/bin/env node
// Input must be reviewed build output, never bytes fetched from a release mirror.
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(import.meta.url);
const { assets, releasePins } = require('../npm/release-integrity.js');
const { version } = require('../package.json');
const args = process.argv.slice(2);
const check = args[0] === '--check';
if (check) args.shift();
const [directory, target] = args;
if (!directory || args.length > 2 || (!check && args.length !== 1) ||
    (target && !['aarch64-unknown-linux-gnu', 'x86_64-unknown-linux-gnu'].includes(target))) {
  throw new Error('usage: node scripts/update_release_pins.mjs [--check] <trusted-build-artifact-directory> [target (check only)]');
}
const digest = asset => createHash('sha256').update(readFileSync(join(directory, asset))).digest('hex');
if (check) {
  const pins = releasePins(version);
  if (readFileSync(join(root, 'npm/release-sha256.txt'), 'utf8') !==
      readFileSync(join(root, 'plugins/computer-use-linux/release-sha256.txt'), 'utf8')) {
    throw new Error('npm and plugin release pins differ');
  }
  for (const asset of assets.filter(asset => !target || asset.endsWith(`-${target}`))) {
    if (digest(asset) !== pins.get(asset)) throw new Error(`build artifact differs from committed release pin: ${asset}`);
  }
  process.exit(0);
}
const pins = assets.sort().map((asset) => {
  return `${version} ${digest(asset)} ${asset}\n`;
}).join('');
// Read every input successfully before replacing either copy.
for (const destination of ['npm/release-sha256.txt', 'plugins/computer-use-linux/release-sha256.txt']) {
  writeFileSync(join(root, destination), pins);
}
