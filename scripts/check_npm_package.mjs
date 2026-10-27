#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { releasePins } = require('../npm/release-integrity.js');
releasePins(require('../package.json').version);
if (readFileSync('npm/release-sha256.txt', 'utf8') !==
    readFileSync('plugins/computer-use-linux/release-sha256.txt', 'utf8')) {
	throw new Error('npm and plugin release pins differ');
}

const requiredFiles = [
	"npm/install.js",
	"npm/release-integrity.js",
	"npm/release-sha256.txt",
	"pi/extension/index.ts",
	"pi/extension/generated-tools.ts",
	"pi/extension/mcp-client.bundle.cjs",
	"pi/extension/THIRD_PARTY_NOTICES.txt",
];
const maxUnpackedBytes = 750_000;

const result = spawnSync(
	"npm",
	["pack", "--dry-run", "--json"],
	{ encoding: "utf8" },
);
if (result.status !== 0) {
	process.stderr.write(result.stderr);
	process.exit(result.status ?? 1);
}

const pack = JSON.parse(result.stdout)[0];
const paths = new Set(pack.files.map((file) => file.path));
const missing = requiredFiles.filter((path) => !paths.has(path));
if (missing.length > 0) {
	throw new Error(`npm package is missing required Pi files: ${missing.join(", ")}`);
}
if (pack.unpackedSize > maxUnpackedBytes) {
	throw new Error(
		`npm package unpacked size ${pack.unpackedSize} exceeds ${maxUnpackedBytes} bytes`,
	);
}

console.log(
	`npm package OK: ${pack.files.length} files, ${pack.size} bytes packed, ` +
		`${pack.unpackedSize} bytes unpacked`,
);
