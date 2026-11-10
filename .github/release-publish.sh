#!/bin/sh
# Publishes the files in dist/ as a GitHub or Gitea release.
#   release-publish.sh release   the pushed tag, as a pre-release
#   release-publish.sh nightly   the one "nightly" pre-release, replaced with this commit
# Needs TOKEN and the GITHUB_* variables both forges set. GitHub and Gitea differ in
# where the API lives, how an asset is uploaded and how a tag is deleted.
set -eu

if [ "$GITHUB_SERVER_URL" = "https://github.com" ]; then
  API="https://api.github.com/repos/$GITHUB_REPOSITORY"
  UPLOADS="https://uploads.github.com/repos/$GITHUB_REPOSITORY"
  github=1
else
  API="$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY"
  github=
fi
auth="Authorization: token $TOKEN"

create() {
  curl -fsS -X POST "$API/releases" -H "$auth" -H "Content-Type: application/json" \
    -d "$1" | jq -r .id
}

upload() {
  name=$(basename "$2")
  if [ -n "$github" ]; then
    curl -fsS -X POST "$UPLOADS/releases/$1/assets?name=$name" -H "$auth" \
      -H "Content-Type: application/octet-stream" --data-binary "@$2" >/dev/null
  else
    curl -fsS -X POST "$API/releases/$1/assets?name=$name" -H "$auth" \
      -F "attachment=@$2" >/dev/null
  fi
}

case "${1:-}" in
  release)
    id=$(create "{\"tag_name\":\"$GITHUB_REF_NAME\",\"name\":\"$GITHUB_REF_NAME\",\"prerelease\":true}")
    ;;
  nightly)
    # One rolling release: drop the last nightly and its tag, then make it again here.
    old=$(curl -s -H "$auth" "$API/releases/tags/nightly" | jq -r '.id // empty')
    if [ -n "$old" ]; then
      curl -fsS -X DELETE -H "$auth" "$API/releases/$old"
    fi
    if [ -n "$github" ]; then
      curl -s -X DELETE -H "$auth" "$API/git/refs/tags/nightly" || true
    else
      curl -s -X DELETE -H "$auth" "$API/tags/nightly" || true
    fi
    short=$(echo "$GITHUB_SHA" | cut -c1-7)
    id=$(create "{\"tag_name\":\"nightly\",\"target_commitish\":\"$GITHUB_SHA\",\"name\":\"nightly ($short)\",\"body\":\"Built from $GITHUB_SHA. Replaced on every push to main.\",\"prerelease\":true}")
    ;;
  *)
    echo "usage: release-publish.sh release|nightly" >&2
    exit 2
    ;;
esac

for f in dist/*; do
  upload "$id" "$f"
done
