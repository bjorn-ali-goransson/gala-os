#!/usr/bin/env bash
# The update feed's signing keys for a Release job (ROADMAP R8; WORKFLOW
# "The update feed"). They live in the private R2 bucket $KEYS_BUCKET as the
# object `feed-keys` (a GALA_FEED_KEYS value), never in the public feed
# bucket. The first publish against an empty feed makes them there; every
# later run reads them. They reach the job's later steps as GALA_FEED_KEYS,
# each seed masked in the log first.
#
#   feed-keys.sh publish   read the keys, or make them for an empty feed
#   feed-keys.sh sign      read the keys (refresh, renew-root)
#
# Needs FEED_URL, FEED_BUCKET, KEYS_BUCKET, AWS_ENDPOINT_URL and the R2
# credentials (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY) in the environment.
set -euo pipefail

mode=${1:?usage: feed-keys.sh publish|sign}
fail() { echo "::error::feed keys: $*"; exit 1; }

# The repository's settings this job needs, named when missing (an empty
# variable shows up as `s3://` or `https://.r2...`, not as an error).
[ "$AWS_ENDPOINT_URL" != "https://.r2.cloudflarestorage.com" ] ||
  fail "the variable R2_ACCOUNT_ID is not set (Settings, Environments, release)"
[ "$FEED_BUCKET" != "s3://" ] ||
  fail "the variable R2_BUCKET is not set (the public feed bucket)"
[ "$KEYS_BUCKET" != "s3://" ] ||
  fail "the variable R2_KEYS_BUCKET is not set (a private bucket for the keys)"
[ -n "${AWS_ACCESS_KEY_ID:-}" ] && [ -n "${AWS_SECRET_ACCESS_KEY:-}" ] ||
  fail "the secrets R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY are not set"
[ "$FEED_BUCKET" != "$KEYS_BUCKET" ] ||
  fail "R2_KEYS_BUCKET is the public feed bucket: the keys need a private one"

keys=$(mktemp)
err=$(mktemp)
trap 'rm -f "$keys" "$err"' EXIT

if aws s3 cp "$KEYS_BUCKET/feed-keys" "$keys" --only-show-errors 2>"$err"; then
  echo "feed keys: read from $KEYS_BUCKET/feed-keys"
else
  grep -q "(404)" "$err" || { cat "$err" >&2; fail "reading $KEYS_BUCKET/feed-keys failed"; }
  # No keys. Only a publish against an empty feed may make them: a feed
  # with a root was signed by keys that are gone, and new ones would be
  # refused by every client that took that root.
  [ "$mode" = publish ] ||
    fail "$KEYS_BUCKET/feed-keys does not exist; the first publish makes it"
  root=$(curl -sS -o /dev/null -w '%{http_code}' "$FEED_URL/1.root.json")
  case $root in
    404) ;;
    200) fail "$FEED_URL has a root but $KEYS_BUCKET/feed-keys is missing: the keys are lost (put back a copy; a new root 1 needs a new, empty feed bucket)" ;;
    *) fail "$FEED_URL/1.root.json answered HTTP $root: cannot tell whether the feed is empty" ;;
  esac
  # The key ids go to the log (stderr); the seeds only to the file.
  cargo xtask feed keygen > "$keys"
  # Never over keys another run made meanwhile.
  aws s3api put-object --bucket "${KEYS_BUCKET#s3://}" --key feed-keys \
    --body "$keys" --if-none-match '*' > /dev/null ||
    fail "storing new keys in $KEYS_BUCKET/feed-keys failed (made by another run?)"
  echo "feed keys: made for the empty feed and stored in $KEYS_BUCKET/feed-keys"
  echo "New feed keys made and stored in \`$KEYS_BUCKET/feed-keys\` (keep a copy offline)." >> "$GITHUB_STEP_SUMMARY"
fi

# Mask every seed before the value goes anywhere a step could print it.
while read -r role seed; do
  case $role in ''|'#'*) continue ;; esac
  [ -n "${seed:-}" ] || fail "feed-keys: a line for $role has no seed"
  echo "::add-mask::$seed"
done < "$keys"
{
  echo "GALA_FEED_KEYS<<GALA_FEED_KEYS_END"
  cat "$keys"
  echo "GALA_FEED_KEYS_END"
} >> "$GITHUB_ENV"
