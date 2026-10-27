'use strict';

const fs = require('node:fs');
const path = require('node:path');

const assets = ['aarch64', 'x86_64'].flatMap((arch) =>
  ['computer-use-linux', 'computer-use-linux-cosmic'].map((name) =>
    `${name}-${arch}-unknown-linux-gnu`));

function releasePins(version) {
  const pins = new Map();
  const text = fs.readFileSync(path.join(__dirname, 'release-sha256.txt'), 'utf8');
  for (const line of text.trimEnd().split('\n')) {
    const match = line.match(/^(\S+) ([a-f0-9]{64}) (\S+)$/);
    if (!match || match[1] !== version || !assets.includes(match[3]) || pins.has(match[3])) {
      throw new Error('invalid or version-mismatched release pins');
    }
    pins.set(match[3], match[2]);
  }
  if (pins.size !== assets.length) throw new Error('missing release pins');
  return pins;
}

module.exports = { assets, releasePins };
