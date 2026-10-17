#!/usr/bin/env bash
#
# Release-path gate logic for AgentSprite (pxl).
#
# WHY THIS FILE EXISTS
# --------------------
# .github/workflows/release.yml runs only on a `v*` tag push, so nothing written inline in it can be exercised by
# branch CI. The fragile shell therefore lives here, where tests/release_gates.rs drives every function through its
# pass path and its fail path on every push (the `release-gates` job in ci.yml), and the workflow steps are thin
# calls into these functions.
#
# Invoke either way:
#   scripts/release_gates.sh <function> [args...]     # from a workflow step
#   source scripts/release_gates.sh                    # from the unit test
#
# THE COMMAND-SUBSTITUTION TRAP. `exit 1` inside `$( )` kills only the substitution subshell, so the enclosing
# command still succeeds and `set -e` sees rc 0. THE RULE HERE: a function either COMPUTES A VALUE (prints to stdout,
# decides nothing) or ASSERTS A CONDITION (returns non-zero on failure, never called from inside `$( )` of another
# command). No function in this file may call `exit`; the unit test enforces that.
#
# Errors are GitHub `::error::` annotations on stderr, so they show up on the run page.

# EMIT. Append KEY=VALUE to the step-output file (stdout when GITHUB_OUTPUT is unset, for local runs).
gh_output() {
  printf '%s=%s\n' "$1" "$2" >> "${GITHUB_OUTPUT:-/dev/stdout}"
}

# COMPUTE. The version declared in a Cargo manifest: the first unindented `version = "..."` line, which in this
# single-package manifest is [package] version. (assert_tag_matches_manifest is the backstop if that ever moves.)
extract_version() {
  local manifest="$1" line
  line=$(grep -m 1 '^version' "$manifest") || return 1
  printf '%s\n' "$line" | cut -d '"' -f 2
}

# ASSERT. N.N.N (prefix-anchored, so 1.0.0-rc.1 passes and 1.2 / abc / empty do not).
assert_version_shape() {
  local version="${1:-}"
  if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
    printf '::error::malformed version %s, expected N.N.N (e.g. 0.1.0)\n' "'${version}'" >&2
    return 1
  fi
}

# COMPUTE. v0.1.0 -> 0.1.0
version_from_ref() {
  printf '%s\n' "${1#v}"
}

# ASSERT. The ref may publish: a TAG named v<N.N.N>. On any other ref GITHUB_REF_NAME is a branch name, the
# archives would be named pxl-main-<target> and the release step would create a release named after the branch,
# all green. The workflow's `on:` lists only the tag push; this is the backstop in every job that uses the ref.
#
#   assert_publish_ref <ref_name> <ref_type> [event_name]
assert_publish_ref() {
  local ref_name="${1:-}" ref_type="${2:-}" event="${3:-}"
  if [ "$ref_type" != "tag" ]; then
    printf '::error::refusing to run the publish path on %s: ref %s is a %s, not a tag. Push a v<N.N.N> tag instead.\n' \
      "'${event:-this event}'" "'${ref_name}'" "'${ref_type:-unknown}'" >&2
    return 1
  fi
  if [[ ! "$ref_name" =~ ^v[0-9]+\.[0-9]+\.[0-9]+ ]]; then
    printf '::error::refusing to run the publish path on tag %s: a release tag must be v<N.N.N> (e.g. v0.1.0).\n' \
      "'${ref_name}'" >&2
    return 1
  fi
  printf 'publishing from tag %s (event %s)\n' "$ref_name" "'${event:-unknown}'" >&2
}

# ASSERT. On a tag build the manifest version must equal the tag. Without this, tagging v0.1.1 with the manifest at
# 0.1.0 ships 0.1.0 binaries (`pxl --version` says 0.1.0) under a 0.1.1 release, all green.
#
#   assert_tag_matches_manifest <manifest_version> <ref_name> <ref_type>
assert_tag_matches_manifest() {
  local version="${1:-}" ref_name="${2:-}" ref_type="${3:-}" tag_version
  if [ "$ref_type" != "tag" ]; then
    printf 'ref %s is a %s, not a tag: tag/manifest gate not applicable\n' "'${ref_name}'" "'${ref_type:-unknown}'" >&2
    return 0
  fi
  tag_version=$(version_from_ref "$ref_name")
  if [ "$version" != "$tag_version" ]; then
    printf '::error::tag/manifest skew: tag %s wants version %s but Cargo.toml declares %s.\n' \
      "$ref_name" "'${tag_version}'" "'${version}'" >&2
    return 1
  fi
}

# ASSERT. The Claude Code plugin manifest carries the same version as Cargo.toml (the skills it ships are the ones
# the binary embeds, and a stamped skill is compared with `pxl --version`).
#
#   assert_plugin_version <manifest_version> <plugin.json>
assert_plugin_version() {
  local version="${1:-}" plugin="${2:-.claude-plugin/plugin.json}" pv
  pv=$(grep -m 1 '"version"' "$plugin" | cut -d '"' -f 4) || pv=""
  if [ "$pv" != "$version" ]; then
    printf '::error::%s declares version %s but Cargo.toml declares %s\n' "$plugin" "'${pv}'" "'${version}'" >&2
    return 1
  fi
}

# COMPUTE. The section body of `## [<version>]` in a changelog, up to the next `## [`.
extract_changelog_section() {
  local version="$1" changelog="$2"
  awk -v v="$version" 'index($0, "## [" v "]") == 1 {found=1; next} /^## \[/ {if (found) exit} found' "$changelog"
}

# EMIT / ASSERT. Write the release notes for <version> to <notes_path> and set `has_notes`. On a tag build a missing
# or empty section is fatal: falling back to auto-generated notes would hide an unpromoted CHANGELOG.
#
#   changelog_notes <version> <changelog> <notes_path> [ref_type]
changelog_notes() {
  local version="$1" changelog="$2" notes_path="$3" ref_type="${4:-}" notes
  notes=$(extract_changelog_section "$version" "$changelog")
  if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
    if [ "$ref_type" = "tag" ]; then
      printf '::error::%s has no non-empty "## [%s]" section but this is a tag build. Add the section and re-tag.\n' \
        "$changelog" "$version" >&2
      return 1
    fi
    printf 'no "## [%s]" section in %s and ref_type is %s, not a tag\n' "$version" "$changelog" "'${ref_type:-unset}'" >&2
    gh_output has_notes false
    return 0
  fi
  printf '%s\n' "$notes" > "$notes_path"
  gh_output has_notes true
}

# COMPUTE. One target triple per line from the `target:` keys of the release-binaries matrix in release.yml, in
# file order. The expected archive set is derived from the same matrix that builds it, so adding or dropping a leg
# cannot leave the artifact gate checking a stale constant.
#
#   rc 0 any legs printed; rc 1 unreadable file or no legs (::error:: on stderr).
release_targets() {
  local workflow="${1:-.github/workflows/release.yml}" out
  if [ ! -f "$workflow" ]; then
    printf '::error::workflow %s not found\n' "'${workflow}'" >&2
    return 1
  fi
  out=$(sed -n 's/^[[:space:]]*target:[[:space:]]*\([A-Za-z0-9_.-][A-Za-z0-9_.-]*\)[[:space:]]*$/\1/p' "$workflow")
  if [ -z "$out" ]; then
    printf '::error::no "target:" legs found in %s\n' "'${workflow}'" >&2
    return 1
  fi
  printf '%s\n' "$out"
}

# COMPUTE. pxl-<tag>-<target>.tar.gz, or .zip for Windows targets.
archive_name() {
  local tag="$1" target="$2" ext="tar.gz"
  case "$target" in *windows*) ext="zip" ;; esac
  printf 'pxl-%s-%s.%s\n' "$tag" "$target" "$ext"
}

# ASSERT. The release directory holds exactly one archive and one .sha256 per matrix leg, nothing else, and every
# checksum file names its archive. A partial set must never be attached to a Release.
#
#   assert_artifact_set <dir> <tag> [workflow]
assert_artifact_set() {
  local dir="${1:-out}" tag="${2:-}" workflow="${3:-.github/workflows/release.yml}"
  local targets t a failures=0 expected="" f n

  if [ ! -d "$dir" ]; then
    printf '::error::artifact directory %s does not exist\n' "'${dir}'" >&2
    return 1
  fi
  if ! targets=$(release_targets "$workflow"); then
    printf '::error::the expected archive set could not be derived from %s\n' "'${workflow}'" >&2
    return 1
  fi
  for t in $targets; do
    a=$(archive_name "$tag" "$t")
    expected="$expected$a"$'\n'"$a.sha256"$'\n'
    if [ ! -s "$dir/$a" ]; then
      printf '::error::missing or empty archive %s\n' "'$a'" >&2
      failures=$((failures + 1))
    fi
    if [ ! -s "$dir/$a.sha256" ]; then
      printf '::error::missing or empty checksum %s\n' "'$a.sha256'" >&2
      failures=$((failures + 1))
    elif ! grep -q "$a" "$dir/$a.sha256"; then
      printf '::error::%s does not name %s\n' "'$a.sha256'" "'$a'" >&2
      failures=$((failures + 1))
    fi
  done
  n=0
  for f in "$dir"/*; do
    [ -e "$f" ] || continue
    n=$((n + 1))
    if ! printf '%s' "$expected" | grep -qxF "$(basename "$f")"; then
      printf '::error::unexpected file %s in %s\n' "'$(basename "$f")'" "'${dir}'" >&2
      failures=$((failures + 1))
    fi
  done
  if [ "$failures" -ne 0 ]; then
    printf '::error::artifact set incomplete: %d problem(s) over %d file(s). Refusing to publish a partial release.\n' "$failures" "$n" >&2
    return 1
  fi
  printf 'artifact set complete: %d files for %d targets\n' "$n" "$(printf '%s\n' "$targets" | wc -l | tr -d ' ')" >&2
}

# Allowlisted so a typo (or an injected argument) cannot run an arbitrary command through this entry point.
main() {
  local cmd="${1:-}"
  if [ -z "$cmd" ]; then
    printf 'usage: release_gates.sh <function> [args...]\n' >&2
    return 2
  fi
  shift
  case "$cmd" in
    extract_version | assert_version_shape | version_from_ref | assert_publish_ref | \
      assert_tag_matches_manifest | assert_plugin_version | extract_changelog_section | changelog_notes | \
      release_targets | archive_name | assert_artifact_set)
      "$cmd" "$@"
      ;;
    *)
      printf '::error::unknown release_gates.sh command: %s\n' "$cmd" >&2
      return 2
      ;;
  esac
}

if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  set -euo pipefail
  main "$@"
fi
