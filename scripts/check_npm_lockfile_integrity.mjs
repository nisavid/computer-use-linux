#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/**
 * Validate a project lock before installing executable dependencies.
 * @param {object} lock Parsed npm version 3 lockfile, including its packages map.
 * @returns {void}
 * @throws {Error} If remote packages lack explicit URLs or authoritative SHA-512 pins.
 */
export function checkLockfileIntegrity(lock) {
	if (lock.lockfileVersion !== 3 || !lock.packages ||
		typeof lock.packages !== "object" || Array.isArray(lock.packages)) {
		throw new Error("expected a version 3 npm lockfile with packages");
	}
	const errors = [];
	for (const [path, entry] of Object.entries(lock.packages)) {
		// npm 11 reloads a dependency's published shrinkwrap during ci, replacing
		// even complete outer integrity metadata with the publisher's metadata.
		if (entry.hasShrinkwrap || entry._hasShrinkwrap) {
			errors.push(`${path}: hasShrinkwrap bypasses the checked-in dependency integrity`);
		}
		// A registry entry may omit resolved; npm then fetches name@version.
		// Exempt only the root and explicitly local dependencies.
		if (path === "" || entry.link === true ||
			(typeof entry.resolved === "string" && entry.resolved.startsWith("file:"))) continue;
		if (typeof entry.resolved !== "string" || !/^https?:\/\//i.test(entry.resolved)) {
			errors.push(`${path}: remote tarball requires an explicit HTTP(S) resolved URL`);
		}
		const digest = typeof entry.integrity === "string"
			? /^sha512-([A-Za-z0-9+/]{86}==)$/.exec(entry.integrity)?.[1]
			: undefined;
		if (!digest || Buffer.from(digest, "base64").toString("base64") !== digest) {
			errors.push(`${path}: remote tarball requires a valid SHA-512 integrity digest`);
		}
	}
	if (errors.length) throw new Error(errors.join("\n"));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
	const paths = process.argv.slice(2);
	if (!paths.length) throw new Error("usage: check_npm_lockfile_integrity.mjs <package-lock.json> ...");
	for (const path of paths) {
		checkLockfileIntegrity(JSON.parse(readFileSync(path, "utf8")));
		console.log(`${path}: remote dependency integrity OK`);
	}
}
