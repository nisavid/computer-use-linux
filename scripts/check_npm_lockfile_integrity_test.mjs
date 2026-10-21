import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";
import { checkLockfileIntegrity } from "./check_npm_lockfile_integrity.mjs";

const integrity = (bytes) => `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
const validDigest = integrity("original package");
const lockWith = (entry) => ({ lockfileVersion: 3, packages: { "node_modules/alias/node_modules/child": entry } });

test("checks every remote entry, including nested aliases and optional dependencies", () => {
	for (const resolved of ["https://registry.example/child.tgz", "HTTP://registry.example/child.tgz", undefined]) {
		for (const invalid of [undefined, "", "sha512-invalid", `sha512-${"A".repeat(85)}B==`, "sha1-abc"]) {
			assert.throws(() => checkLockfileIntegrity(lockWith({ resolved, integrity: invalid, optional: true })), /SHA-512/);
		}
		assert.doesNotThrow(() => checkLockfileIntegrity(lockWith({ resolved, integrity: validDigest })));
	}
	assert.doesNotThrow(() => checkLockfileIntegrity(lockWith({ resolved: "file:../workspace", link: true })));
	assert.doesNotThrow(() => checkLockfileIntegrity(lockWith({ resolved: "file:../local.tgz" })));
	assert.doesNotThrow(() => checkLockfileIntegrity({ lockfileVersion: 3, packages: { "": { name: "root" } } }));
	assert.throws(() => checkLockfileIntegrity({ lockfileVersion: 1 }), /version 3/);
});

test("rejects shrinkwrap reloads even when the outer lock is integrity-complete", () => {
	for (const flag of ["hasShrinkwrap", "_hasShrinkwrap"]) {
		assert.throws(() => checkLockfileIntegrity(lockWith({
			resolved: "https://registry.example/parent.tgz", integrity: validDigest, [flag]: true,
		})), /hasShrinkwrap/);
	}
});

test("checked-in Pi lock satisfies the pre-install policy", () => {
	checkLockfileIntegrity(JSON.parse(readFileSync(new URL("../pi/package-lock.json", import.meta.url), "utf8")));
});

test("npm ci enforces outer integrity despite an incomplete published shrinkwrap", { timeout: 30_000 }, async (t) => {
	const dir = mkdtempSync(join(tmpdir(), "npm-lock-integrity-"));
	const tarballs = new Map();
	const server = createServer((request, response) => {
		const bytes = tarballs.get(request.url);
		response.writeHead(bytes ? 200 : 404, { "Content-Type": "application/octet-stream" });
		response.end(bytes);
	});
	t.after(async () => {
		await new Promise((resolve) => server.close(resolve));
		rmSync(dir, { recursive: true, force: true });
	});
	await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
	const base = `http://127.0.0.1:${server.address().port}`;
	function pack(name, files) {
		const root = join(dir, name);
		mkdirSync(join(root, "package"), { recursive: true });
		for (const [path, content] of Object.entries(files)) {
			writeFileSync(join(root, "package", path), typeof content === "string" ? content : JSON.stringify(content));
		}
		const output = join(dir, `${name}.tgz`);
		execFileSync("tar", ["-czf", output, "-C", root, "package"]);
		return readFileSync(output);
	}
	const child = pack("child", { "package.json": { name: "child", version: "1.0.0" }, "index.js": 'module.exports = "original";\n' });
	const changedChild = pack("changed-child", { "package.json": { name: "child", version: "1.0.0" }, "index.js": 'module.exports = "substituted";\n' });
	const childEntry = { version: "1.0.0", resolved: `${base}/child.tgz` };
	const parentManifest = { name: "parent", version: "1.0.0", dependencies: { child: "1.0.0" } };
	const parent = pack("parent", {
		"package.json": parentManifest,
		"npm-shrinkwrap.json": { name: "parent", version: "1.0.0", lockfileVersion: 3,
			packages: { "": parentManifest, "node_modules/child": childEntry } },
	});
	tarballs.set("/parent.tgz", parent);
	const manifest = { name: "fixture", version: "1.0.0", dependencies: { parent: "1.0.0" } };
	const lock = { name: "fixture", version: "1.0.0", lockfileVersion: 3, packages: {
		"": manifest,
		// Deliberately omit hasShrinkwrap, as in the corrected Pi lock.
		"node_modules/parent": { ...parentManifest, resolved: `${base}/parent.tgz`, integrity: integrity(parent) },
		"node_modules/parent/node_modules/child": { ...childEntry, integrity: integrity(child) },
	} };
	checkLockfileIntegrity(lock);
	for (const [name, bytes] of [["original", child], ["substituted", changedChild], ["registry-substitution", changedChild]]) {
		tarballs.set("/child.tgz", bytes);
		const candidate = structuredClone(lock);
		if (name === "registry-substitution") {
			delete candidate.packages["node_modules/parent/node_modules/child"].resolved;
			tarballs.set("/child", JSON.stringify({ name: "child", "dist-tags": { latest: "1.0.0" },
				versions: { "1.0.0": { name: "child", version: "1.0.0", dist: { tarball: `${base}/child.tgz` } } } }));
		}
		checkLockfileIntegrity(candidate);
		const project = join(dir, name);
		mkdirSync(project);
		writeFileSync(join(project, "package.json"), JSON.stringify(manifest));
		writeFileSync(join(project, "package-lock.json"), JSON.stringify(candidate));
		const install = () => promisify(execFile)("npm", ["ci", "--ignore-scripts", "--no-audit", "--no-fund",
			"--fetch-retries=0", "--cache", join(project, "cache"), "--registry", base], { cwd: project, timeout: 15_000 });
		if (name === "original") {
			await install();
			assert.equal(readFileSync(join(project, "node_modules/parent/node_modules/child/index.js"), "utf8"), 'module.exports = "original";\n');
			assert.equal(readFileSync(join(project, "package-lock.json"), "utf8"), JSON.stringify(lock));
		} else {
			await assert.rejects(install, (error) => /EINTEGRITY/.test(error.stderr));
		}
	}
});
