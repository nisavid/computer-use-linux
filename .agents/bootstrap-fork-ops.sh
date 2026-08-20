#!/usr/bin/env bash

set -euo pipefail

readonly fork_https_url="https://github.com/nisavid/computer-use-linux.git"
readonly upstream_https_url="https://github.com/agent-sh/computer-use-linux.git"
readonly upstream_main_ref="refs/remotes/upstream/main"

usage() {
  printf '%s\n' 'Usage: .agents/bootstrap-fork-ops.sh'
  printf '%s\n' 'Provision and verify the read-only upstream remote for Fork Ops.'
}

die() {
  printf 'bootstrap-fork-ops: %s\n' "$1" >&2
  exit 1
}

is_fork_url() {
  local remote_url="$1"
  while [[ "$remote_url" == */ ]]; do
    remote_url="${remote_url%/}"
  done

  case "$remote_url" in
    "https://github.com/nisavid/computer-use-linux" | "$fork_https_url" | \
      "git@github.com:nisavid/computer-use-linux" | \
      "git@github.com:nisavid/computer-use-linux.git" | \
      "ssh://git@github.com/nisavid/computer-use-linux" | \
      "ssh://git@github.com/nisavid/computer-use-linux.git")
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

is_upstream_url() {
  local remote_url="$1"
  while [[ "$remote_url" == */ ]]; do
    remote_url="${remote_url%/}"
  done

  case "$remote_url" in
    "https://github.com/agent-sh/computer-use-linux" | "$upstream_https_url" | \
      "git@github.com:agent-sh/computer-use-linux" | \
      "git@github.com:agent-sh/computer-use-linux.git" | \
      "ssh://git@github.com/agent-sh/computer-use-linux" | \
      "ssh://git@github.com/agent-sh/computer-use-linux.git")
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

main() {
  if (($# > 0)); then
    case "$1" in
      -h | --help)
        usage
        return 0
        ;;
      *)
        usage >&2
        return 2
        ;;
    esac
  fi

  command -v git >/dev/null 2>&1 || die 'git is required'

  local repo_root origin_url upstream_url upstream_sha
  local -a configured_push_urls=()
  repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" ||
    die 'run this script from a computer-use-linux checkout'

  origin_url="$(git -C "$repo_root" config --local --get remote.origin.url 2>/dev/null)" ||
    die 'origin is missing'
  is_fork_url "$origin_url" ||
    die "origin does not identify nisavid/computer-use-linux: $origin_url"

  if upstream_url="$(git -C "$repo_root" config --local --get remote.upstream.url 2>/dev/null)"; then
    is_upstream_url "$upstream_url" ||
      die "upstream does not identify agent-sh/computer-use-linux: $upstream_url"
  else
    git -C "$repo_root" remote add upstream "$upstream_https_url"
  fi

  git -C "$repo_root" config --local --replace-all remote.upstream.pushurl ''
  git -C "$repo_root" config --local --add remote.upstream.pushurl DISABLED
  git -C "$repo_root" fetch --no-tags upstream \
    "+refs/heads/main:$upstream_main_ref"

  mapfile -t configured_push_urls < <(
    git -C "$repo_root" config --local --get-all remote.upstream.pushurl
  )
  ((${#configured_push_urls[@]} == 2)) &&
    [[ -z "${configured_push_urls[0]}" ]] &&
    [[ "${configured_push_urls[1]}" == DISABLED ]] ||
    die 'upstream push URL config is not safely disabled'
  upstream_sha="$(git -C "$repo_root" rev-parse --verify "$upstream_main_ref^{commit}")" ||
    die 'upstream main was not fetched'

  printf 'Fork Ops upstream ready at %s; pushes are disabled.\n' "$upstream_sha"
}

main "$@"
