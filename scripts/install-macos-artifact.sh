#!/usr/bin/env bash
set -euo pipefail

repo="${ACCESSIBILITY_CLI_REPO:-ealmloff/accessibility-cli}"
workflow="${ACCESSIBILITY_CLI_WORKFLOW:-pr-build.yml}"
out_dir="dist/artifacts"
selector=""
selector_value=""

usage() {
  cat <<'USAGE'
Usage: scripts/install-macos-artifact.sh [options]

Downloads all unexpired GitHub Actions artifacts from the selected workflow run.

Options:
  --main                  Download artifacts from the latest main run.
  --commit sha            Download artifacts built for a specific commit.
  --pr number             Download artifacts built for a PR.
  --repo owner/name       GitHub repository. Defaults to ACCESSIBILITY_CLI_REPO
                          or ealmloff/accessibility-cli.
  --workflow file         Workflow file or id. Defaults to pr-build.yml.
  --out-dir path          Output directory. Defaults to dist/artifacts.
  -h, --help              Show this help.

Auth:
  Set GITHUB_TOKEN or GH_TOKEN for private repositories. If neither is set and
  gh is installed, this script will try `gh auth token`.
USAGE
}

set_selector() {
  if [[ -n "$selector" ]]; then
    echo "choose only one of --main, --commit, or --pr" >&2
    exit 2
  fi

  selector="$1"
  selector_value="${2:-}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --main)
      set_selector main
      shift
      ;;
    --commit)
      set_selector commit "$2"
      shift 2
      ;;
    --pr)
      set_selector pr "$2"
      shift 2
      ;;
    --repo)
      repo="$2"
      shift 2
      ;;
    --workflow)
      workflow="$2"
      shift 2
      ;;
    --out-dir)
      out_dir="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$selector" ]]; then
  echo "choose one of --main, --commit, or --pr" >&2
  usage >&2
  exit 2
fi

for cmd in curl python3 unzip awk find chmod; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "$cmd is required" >&2
    exit 1
  fi
done

token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
if [[ -z "$token" ]] && command -v gh >/dev/null 2>&1; then
  token="$(gh auth token 2>/dev/null || true)"
fi

api_headers=(
  -H "Accept: application/vnd.github+json"
  -H "X-GitHub-Api-Version: 2022-11-28"
)

if [[ -n "$token" ]]; then
  api_headers+=(-H "Authorization: Bearer $token")
fi

urlencode() {
  python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
}

api_get() {
  curl -fsSL "${api_headers[@]}" "$1"
}

run_ids_from_json() {
  python3 -c '
import json, sys
for run in json.load(sys.stdin).get("workflow_runs", []):
    print(run["id"])
'
}

artifact_rows_from_json() {
  python3 -c '
import json, sys
for artifact in json.load(sys.stdin).get("artifacts", []):
    if not artifact.get("expired"):
        print("{}\t{}".format(artifact["id"], artifact["name"]))
'
}

find_run_artifacts() {
  artifact_rows=""
  while IFS= read -r candidate_run_id; do
    if [[ -z "$candidate_run_id" ]]; then
      continue
    fi

    artifacts_json="$(api_get "https://api.github.com/repos/$repo/actions/runs/$candidate_run_id/artifacts?per_page=100")"
    artifact_rows="$(artifact_rows_from_json <<<"$artifacts_json")"
    if [[ -n "$artifact_rows" ]]; then
      run_id="$candidate_run_id"
      break
    fi
  done
}

download_artifact() {
  local artifact_id="$1"
  local artifact_name="$2"
  local headers="$tmpdir/headers-$artifact_id"
  local zip_path="$tmpdir/$artifact_name.zip"
  local download_api_url="https://api.github.com/repos/$repo/actions/artifacts/$artifact_id/zip"
  local status
  local location

  status="$(
    curl -sS -D "$headers" -o /dev/null -w "%{http_code}" "${api_headers[@]}" "$download_api_url"
  )"

  if [[ "$status" != "302" ]]; then
    echo "artifact download request for $artifact_name failed with HTTP $status" >&2
    exit 1
  fi

  location="$(
    python3 -c '
import sys
for line in open(sys.argv[1], encoding="iso-8859-1"):
    if line.lower().startswith("location:"):
        print(line.split(":", 1)[1].strip())
        break
' "$headers"
  )"

  if [[ -z "$location" ]]; then
    echo "GitHub did not return an artifact download location for $artifact_name" >&2
    exit 1
  fi

  curl -fsSL "$location" -o "$zip_path"
  unzip -q -o "$zip_path" -d "$out_dir"
  echo "downloaded $artifact_name"
}

encoded_workflow="$(urlencode "$workflow")"
case "$selector" in
  main)
    runs_url="https://api.github.com/repos/$repo/actions/workflows/$encoded_workflow/runs?branch=main&status=completed&per_page=20"
    run_ids="$(api_get "$runs_url" | run_ids_from_json)"
    ;;
  commit)
    encoded_commit="$(urlencode "$selector_value")"
    runs_url="https://api.github.com/repos/$repo/actions/workflows/$encoded_workflow/runs?head_sha=$encoded_commit&status=completed&per_page=20"
    run_ids="$(api_get "$runs_url" | run_ids_from_json)"
    ;;
  pr)
    pr_json="$(api_get "https://api.github.com/repos/$repo/pulls/$selector_value")"
    pr_head_sha="$(
      python3 -c 'import json, sys; print(json.load(sys.stdin)["head"]["sha"])' <<<"$pr_json"
    )"
    pr_head_ref="$(
      python3 -c 'import json, sys; print(json.load(sys.stdin)["head"]["ref"])' <<<"$pr_json"
    )"
    encoded_pr_head_sha="$(urlencode "$pr_head_sha")"
    encoded_pr_head_ref="$(urlencode "$pr_head_ref")"
    pr_runs_url="https://api.github.com/repos/$repo/actions/workflows/$encoded_workflow/runs?event=pull_request&branch=$encoded_pr_head_ref&status=completed&per_page=20"
    commit_runs_url="https://api.github.com/repos/$repo/actions/workflows/$encoded_workflow/runs?head_sha=$encoded_pr_head_sha&status=completed&per_page=20"
    run_ids="$(
      {
        api_get "$pr_runs_url" | run_ids_from_json
        api_get "$commit_runs_url" | run_ids_from_json
      } | awk '!seen[$0]++'
    )"
    ;;
esac

if [[ -z "$run_ids" ]]; then
  echo "no completed workflow run found for $selector ${selector_value:-main}" >&2
  exit 1
fi

run_id=""
find_run_artifacts <<<"$run_ids"

if [[ -z "$artifact_rows" ]]; then
  echo "no unexpired artifacts found for $selector ${selector_value:-main}" >&2
  exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

mkdir -p "$out_dir"

while IFS=$'\t' read -r artifact_id artifact_name; do
  if [[ -z "$artifact_id" || -z "$artifact_name" ]]; then
    continue
  fi

  download_artifact "$artifact_id" "$artifact_name"
done <<<"$artifact_rows"

find "$out_dir" -maxdepth 1 -type f -name 'accessibility-cli-*' -exec chmod +x {} +

echo "downloaded artifacts from run $run_id to $out_dir"
