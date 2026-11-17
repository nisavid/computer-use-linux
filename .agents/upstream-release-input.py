"""Resolve the stable upstream input for upstream-drift.sh."""

import base64
import json
import os
import re
import subprocess
import sys
from datetime import datetime
from hashlib import sha256
from pathlib import Path
from tempfile import NamedTemporaryFile

try:
    import tomllib
except ModuleNotFoundError:
    sys.exit("upstream-release-input: Python 3.11 or newer is required")

STABLE_TAG = re.compile(
    r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
GITHUB_OID = re.compile(r"[0-9a-f]{40}")


def api(repo, endpoint):
    return json.loads(
        subprocess.check_output(
            ["gh", "api", "--hostname", "github.com", f"repos/{repo}/{endpoint}"]
        )
    )


def git(*args):
    return subprocess.check_output(["git", "--no-replace-objects", *args])


def release_fields(response):
    if not isinstance(response, dict):
        raise TypeError("latest release must be a JSON object")
    release_id, tag = response.get("id"), response.get("tag_name")
    published, body = response.get("published_at"), response.get("body")
    if (
        type(release_id) is not int
        or release_id <= 0
        or not isinstance(tag, str)
        or STABLE_TAG.fullmatch(tag) is None
        or not isinstance(body, str)
    ):
        raise ValueError(
            "latest release lacks a valid ID, stable version tag, or notes body"
        )
    if response.get("draft") is not False or response.get("prerelease") is not False:
        raise ValueError("latest release must be neither draft nor prerelease")
    try:
        if (
            not isinstance(published, str)
            or re.fullmatch(
                r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", published
            )
            is None
        ):
            raise ValueError("timestamp must use canonical UTC format")
        datetime.strptime(published, "%Y-%m-%dT%H:%M:%SZ")
    except (TypeError, ValueError) as error:
        raise ValueError(
            "latest release lacks a valid publication timestamp"
        ) from error
    release = {
        "id": release_id,
        "tag": tag,
        "draft": False,
        "prerelease": False,
        "published_at": published,
        "body": body,
    }
    return release


def observe(repo):
    release = release_fields(api(repo, "releases/latest"))
    tag = release["tag"]
    reference = api(repo, f"git/ref/tags/{tag}")
    if not isinstance(reference, dict) or reference.get("ref") != f"refs/tags/{tag}":
        raise ValueError("GitHub did not return the selected tag ref")
    target = reference.get("object")
    if not isinstance(target, dict):
        raise TypeError("selected tag ref lacks an object")
    oid, kind = target.get("sha"), target.get("type")
    if (
        not isinstance(oid, str)
        or GITHUB_OID.fullmatch(oid) is None
        or kind not in ("tag", "commit")
    ):
        raise ValueError("selected ref must point to a valid Git tag or commit object")
    return release, oid, kind


def resolve(repo, selected_tag, selected_commit, selected_release):
    selected = release_fields(
        json.loads(base64.b64decode(selected_release, validate=True))
    )
    first = observe(repo)
    release, tag_oid, tag_kind = first
    tag = release["tag"]
    if tag != selected_tag or release != selected:
        raise ValueError("latest release changed after selection")
    fetched = git("rev-parse", "--verify", "FETCH_HEAD^{object}").decode().strip()
    if (
        fetched != tag_oid
        or git("cat-file", "-t", tag_oid).decode().strip() != tag_kind
    ):
        raise ValueError(f"fetched tag {tag} disagrees with the GitHub tag identity")
    commit = git("rev-parse", "--verify", f"{tag_oid}^{{commit}}").decode().strip()
    if commit != selected_commit:
        raise ValueError("selected commit changed during resolution")
    cargo = git("cat-file", "blob", f"{commit}:Cargo.toml")
    package = tomllib.loads(cargo.decode("utf-8")).get("package", {})
    version = package.get("version") if isinstance(package, dict) else None
    if version != tag[1:]:
        raise ValueError(f"Cargo package version {version!r} disagrees with tag {tag}")
    if observe(repo) != first:
        raise ValueError("latest release or tag changed during resolution")
    release["body_sha256"] = sha256(release["body"].encode("utf-8")).hexdigest()
    resolver = Path(__file__).resolve()
    return {
        "schema": "computer-use-linux/upstream-release-input/v1",
        "upstream_repository": repo,
        "release": release,
        "source": {
            "tag_object": tag_oid,
            "tag_object_type": tag_kind,
            "commit": commit,
            "cargo_version": version,
            "cargo_sha256": sha256(cargo).hexdigest(),
        },
        "procedure": {
            "resolver_sha256": sha256(resolver.read_bytes()).hexdigest(),
            "drift_entrypoint_sha256": sha256(
                resolver.with_name("upstream-drift.sh").read_bytes()
            ).hexdigest(),
        },
    }


def retain(record, destination):
    staging = None
    try:
        with NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=destination.parent, delete=False
        ) as output:
            staging = Path(output.name)
            json.dump(record, output, ensure_ascii=False, indent=2)
            output.write("\n")
        os.link(staging, destination)
    finally:
        if staging is not None:
            staging.unlink()


def main(repo, selected_tag, selected_commit, destination):
    record = resolve(repo, selected_tag, selected_commit, sys.stdin.buffer.read())
    retain(record, Path(destination))


if __name__ == "__main__":
    try:
        main(*sys.argv[1:])
    except (OSError, TypeError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"upstream-release-input: {error}")
