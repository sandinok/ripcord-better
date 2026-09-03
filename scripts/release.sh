#!/usr/bin/env bash
# Helper: create the GitHub release for a tag and dispatch the build matrix.
# Usage: ./scripts/release.sh v0.1.0
set -euo pipefail
TAG="${1:?usage: release.sh <tag>}"
REPO="sandinok/basalt"
TOKEN=$(sed -n 's|.*://[^:]*:\([^@]*\)@github.com.*|\1|p' ~/.git-credentials 2>/dev/null | head -1)
if [ -z "$TOKEN" ]; then
  echo "No GitHub token found in ~/.git-credentials" >&2
  exit 1
fi
# Create the release (idempotent: ignore 422 already-exists).
curl -sf -X POST "https://api.github.com/repos/$REPO/releases" \
  -H "Authorization: token $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"tag_name\":\"$TAG\",\"name\":\"Basalt $TAG\",\"body\":\"Release notes go here.\",\"prerelease\":false}" \
  || echo "(release may already exist)"
# Dispatch the build workflow.
curl -sf -X POST "https://api.github.com/repos/$REPO/actions/workflows/release.yml/dispatches" \
  -H "Authorization: token $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"ref\":\"main\",\"inputs\":{\"tag\":\"$TAG\"}}"
echo "Build dispatched for $TAG"
