#!/usr/bin/env bash
# The first job of every workflow: decide whether the rest has to run, so CI only runs on changes.
#
#   tools/ci/gate.sh <workflow-file>        (for example release.yml)
#
# Writes `run=true` or `run=false` to $GITHUB_OUTPUT. The other jobs `need: gate` and run only when `needs.gate.outputs.run == 'true'`; a
# skipped workflow is still green. Skips when
#   - a tag run: the release for that tag already has assets, or the release site already serves that version's files
#     (`rusty-wave-<version>-SHA256SUMS` is served): a real release is not built twice, and a tag pushed after a local publish never
#     rebuilds. A manual dry run on the same commit does not count;
#   - any other run: a successful run of the same workflow already exists for this commit (other than this one).
# `FORCE=true` (the `force` input of a manual run) overrides it. If GitHub cannot be asked, it runs (fail open). Drafts are invisible to the
# read-only token, so a tag run whose release exists only as a draft builds again, which is harmless: the release step updates the draft.
#
# Environment (all set by Actions): GH_TOKEN, GITHUB_REPOSITORY, GITHUB_SHA, GITHUB_RUN_ID, GITHUB_REF_TYPE, GITHUB_REF_NAME, GITHUB_OUTPUT, FORCE.
set -uo pipefail

wf="${1:?usage: gate.sh <workflow-file>}"
out="${GITHUB_OUTPUT:-/dev/stdout}"

decide() {
  echo "run=$1" >> "$out"
  echo "gate: run=$1 ($2)"
  exit 0
}

[[ "${FORCE:-}" == "true" ]] && decide true "forced"

if [[ "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
  if ! assets=$(gh api "repos/${GITHUB_REPOSITORY}/releases?per_page=100" --paginate \
      --jq ".[] | select(.tag_name == \"${GITHUB_REF_NAME}\") | .assets | length"); then
    decide true "could not list releases; running to be safe"
  fi
  total=$(awk '{s += $1} END {print s + 0}' <<< "$assets")
  if [[ "$total" -gt 0 ]]; then
    decide false "the release for ${GITHUB_REF_NAME} already has ${total} assets"
  fi
  # The files may already be published on Rusty Bucket's release site: a version is published once, so a tag pushed after that never rebuilds.
  base="${DIST_BASE_URL:-https://software.rustybucket.ai/rusty-wave}"
  ver="${GITHUB_REF_NAME#v}"
  url="${base}/${ver}/rusty-wave-${ver}-SHA256SUMS"
  code=$(curl -s -A 'rw-ci/1 curl' -o /dev/null -I -w '%{http_code}' --max-time 20 "$url" || true)
  if [[ "$code" == "200" ]]; then
    decide false "${ver} is already published at ${base} (rusty-wave-${ver}-SHA256SUMS)"
  fi
  decide true "no release with assets for ${GITHUB_REF_NAME} and nothing published at ${base} (HTTP ${code:-none})"
fi

if ! count=$(gh api "repos/${GITHUB_REPOSITORY}/actions/workflows/${wf}/runs?head_sha=${GITHUB_SHA}&status=success&per_page=100" \
    --jq ".workflow_runs | map(select(.id != ${GITHUB_RUN_ID})) | length"); then
  decide true "could not list runs; running to be safe"
fi
if [[ "${count:-0}" -gt 0 ]]; then
  decide false "${wf} already succeeded ${count} time(s) on ${GITHUB_SHA}; use the force input to run anyway"
fi
decide true "no earlier successful run on ${GITHUB_SHA}"
