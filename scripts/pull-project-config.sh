#!/usr/bin/env bash
# The Action's project-config step (carrick#2233): fetch the configuration
# Carrick holds for this repository before anything reads its services, so the
# install and the scan use the split the cloud runner scans under.
#
# Usage: pull-project-config.sh <scan-path>
# Env:   CARRICK_CLI (the wrapper "Name the resolved Carrick" writes),
#        RUNNER_TEMP, GITHUB_ENV, GITHUB_OUTPUT.
#
# Writes `skip=false` to GITHUB_OUTPUT and `CARRICK_CONFIG=<file>` to
# GITHUB_ENV when the pull wrote its answer, and `skip=true` otherwise; every
# later Carrick step in action.yml is gated on `skip`, so a failed pull runs
# no install, no scan and no upload. Exits 0 either way, so the job stays
# green, and prints nothing: an internal fault is never a line in the
# customer's log. What the command said goes to a file under RUNNER_TEMP.
set -u

scan_path="${1:-.}"
dir="${RUNNER_TEMP:?}/carrick"
out="$dir/config.json"

if mkdir -p "$dir" \
  && env -u CARRICK_CONFIG "${CARRICK_CLI:?}" config pull \
    --workspace "$scan_path" --out "$out" >"$dir/config-pull.log" 2>&1; then
  echo "CARRICK_CONFIG=$out" >>"${GITHUB_ENV:?}"
  echo "skip=false" >>"${GITHUB_OUTPUT:?}"
else
  echo "skip=true" >>"${GITHUB_OUTPUT:?}"
fi
exit 0
