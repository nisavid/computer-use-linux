#!/usr/bin/env bash

set -euo pipefail

readonly fork_repo="nisavid/computer-use-linux"
readonly upstream_repo="agent-sh/computer-use-linux"
readonly default_fork_ref="refs/remotes/origin/main"
readonly default_upstream_ref="refs/remotes/upstream/main"
readonly default_stale_days=14

usage() {
  printf '%s\n' 'Usage: .agents/upstream-drift.sh [--fork-ref REF] [--upstream-ref REF]'
  printf '%s\n' '                                 [--stale-days N] [--report FILE]'
  printf '%s\n' 'Report how far the fork trails upstream main and the latest stable upstream release.'
  printf '%s\n' 'Prints key=value lines; --report also writes a Markdown summary to FILE.'
  printf '%s\n' 'Run .agents/bootstrap-fork-ops.sh first. Requires git and an authenticated gh.'
}

die() {
  printf 'upstream-drift: %s\n' "$1" >&2
  exit 1
}

main() {
  local fork_ref="$default_fork_ref"
  local upstream_ref="$default_upstream_ref"
  local stale_days="$default_stale_days"
  local report_file=''

  while (($# > 0)); do
    case "$1" in
      --fork-ref)
        fork_ref="${2:?--fork-ref needs a value}"
        shift 2
        ;;
      --upstream-ref)
        upstream_ref="${2:?--upstream-ref needs a value}"
        shift 2
        ;;
      --stale-days)
        stale_days="${2:?--stale-days needs a value}"
        shift 2
        ;;
      --report)
        report_file="${2:?--report needs a value}"
        shift 2
        ;;
      -h | --help)
        usage
        return 0
        ;;
      *)
        usage >&2
        return 2
        ;;
    esac
  done

  [[ "$stale_days" =~ ^[0-9]+$ ]] || die "--stale-days must be a whole number: $stale_days"
  command -v git >/dev/null 2>&1 || die 'git is required'
  command -v gh >/dev/null 2>&1 || die 'gh is required'

  local fork_sha upstream_sha
  fork_sha="$(git rev-parse --verify "$fork_ref^{commit}" 2>/dev/null)" ||
    die "fork ref is missing: $fork_ref"
  upstream_sha="$(git rev-parse --verify "$upstream_ref^{commit}" 2>/dev/null)" ||
    die "upstream ref is missing: $upstream_ref (run .agents/bootstrap-fork-ops.sh)"

  local behind ahead
  behind="$(git rev-list --count "$fork_sha..$upstream_sha")"
  ahead="$(git rev-list --count "$upstream_sha..$fork_sha")"

  local now oldest_missing_epoch='' oldest_missing_age_days=0
  now="$(date -u +%s)"
  if ((behind > 0)); then
    oldest_missing_epoch="$(git log --format=%ct "$fork_sha..$upstream_sha" | sort -n | head -n 1)"
    oldest_missing_age_days=$(((now - oldest_missing_epoch) / 86400))
  fi

  # The stable release channel in .agents/fork-ops.toml: GitHub's latest
  # release, which excludes drafts and prereleases.
  local release_tag release_url release_sha release_contained=true
  release_tag="$(gh api "repos/$upstream_repo/releases/latest" --jq .tag_name)" ||
    die "could not read the latest $upstream_repo release"
  release_url="https://github.com/$upstream_repo/releases/tag/$release_tag"
  # Fetch into FETCH_HEAD only, so upstream tags never overwrite fork tags.
  git fetch --quiet --no-tags upstream "refs/tags/$release_tag" ||
    die "could not fetch upstream tag $release_tag"
  release_sha="$(git rev-parse --verify 'FETCH_HEAD^{commit}')"
  local ancestry_status=0
  git merge-base --is-ancestor "$release_sha" "$fork_sha" || ancestry_status=$?
  case "$ancestry_status" in
    0) ;;
    1) release_contained=false ;;
    *) die "could not compare $release_tag with $fork_ref (git exit $ancestry_status)" ;;
  esac

  local fork_release
  fork_release="$(git describe --tags --abbrev=0 "$fork_sha" 2>/dev/null || printf 'none')"

  local status
  if [[ "$release_contained" == false ]] || ((oldest_missing_age_days > stale_days)); then
    status=drifted
  elif ((behind == 0)); then
    status=current
  else
    status=trailing
  fi

  printf 'status=%s\n' "$status"
  printf 'behind=%s\n' "$behind"
  printf 'ahead=%s\n' "$ahead"
  printf 'oldest_missing_age_days=%s\n' "$oldest_missing_age_days"
  printf 'stale_days=%s\n' "$stale_days"
  printf 'fork_sha=%s\n' "$fork_sha"
  printf 'upstream_sha=%s\n' "$upstream_sha"
  printf 'fork_release=%s\n' "$fork_release"
  printf 'release_tag=%s\n' "$release_tag"
  printf 'release_contained=%s\n' "$release_contained"

  [[ -n "$report_file" ]] || return 0

  local oldest_missing='n/a'
  if [[ -n "$oldest_missing_epoch" ]]; then
    oldest_missing="$(date -u -d "@$oldest_missing_epoch" +%Y-%m-%d) ($oldest_missing_age_days days)"
  fi

  # shellcheck disable=SC2016 # Backticks are Markdown code spans.
  {
    printf '| Measure | Value |\n'
    printf '| --- | --- |\n'
    printf '| Status | `%s` |\n' "$status"
    printf '| Upstream commits missing from fork `main` | %s |\n' "$behind"
    printf '| Fork commits not in upstream `main` | %s |\n' "$ahead"
    printf '| Oldest missing upstream commit | %s |\n' "$oldest_missing"
    printf '| Latest stable upstream release | [%s](%s) |\n' "$release_tag" "$release_url"
    printf '| Fork contains that release | %s |\n' "$release_contained"
    printf '| Newest release tag in fork history | `%s` |\n' "$fork_release"
    printf '| Fork `main` | `%s` |\n' "$fork_sha"
    printf '| Upstream `main` | `%s` |\n' "$upstream_sha"
    printf '\n'
    printf 'Compare: https://github.com/%s/compare/main...%s:main\n' \
      "$fork_repo" "${upstream_repo/\//:}"
  } >"$report_file"
}

main "$@"
