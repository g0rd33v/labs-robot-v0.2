#!/usr/bin/env bash
#
# Upload one file to an S3-compatible bucket with AWS Signature V4.
#
#   scripts/s3-put.sh <local-file> <remote-key>
#   scripts/s3-put.sh --head <remote-key>      # exists? print size
#   scripts/s3-put.sh --get <remote-key> <out> # download (for restore drills)
#
# Signed by hand with curl + openssl rather than pulling in an SDK or CLI:
# the project rule is no new dependencies without listing them, and this is
# ~40 lines against a stable, documented signing algorithm.
#
# Credentials come from the macOS keychain and are never written to disk,
# passed on a command line, or echoed. They exist only as shell variables in
# this process.

set -euo pipefail

ENDPOINT="${S3_ENDPOINT:-ams3.digitaloceanspaces.com}"
BUCKET="${S3_BUCKET:-backup-labs-co}"
REGION="${S3_REGION:-ams3}"
SERVICE="s3"

kc() { security find-generic-password -a bender -s "$1" -w 2>/dev/null; }
ACCESS_KEY="${DO_SPACES_KEY:-$(kc DO_SPACES_KEY)}"
SECRET_KEY="${DO_SPACES_SECRET:-$(kc DO_SPACES_SECRET)}"
[ -n "$ACCESS_KEY" ] && [ -n "$SECRET_KEY" ] || {
    echo "!! no S3 credentials (keychain items DO_SPACES_KEY / DO_SPACES_SECRET)" >&2
    exit 1
}

HOST="$BUCKET.$ENDPOINT"
sha256_hex() { openssl dgst -sha256 -binary "$1" | xxd -p -c 256; }
sha256_str() { printf '%s' "$1" | openssl dgst -sha256 -binary | xxd -p -c 256; }
hmac_hex() { printf '%s' "$2" | openssl dgst -sha256 -mac HMAC -macopt "hexkey:$1" -binary | xxd -p -c 256; }

sign_and_send() {
    local method="$1" key="$2" file="${3:-}" outfile="${4:-}"
    local canonical_uri="/$key"
    local amz_date date_stamp payload_hash
    amz_date=$(date -u +%Y%m%dT%H%M%SZ)
    date_stamp=$(date -u +%Y%m%d)

    if [ "$method" = "PUT" ]; then
        payload_hash=$(sha256_hex "$file")
    else
        payload_hash=$(sha256_str "")
    fi

    local canonical_headers="host:$HOST
x-amz-content-sha256:$payload_hash
x-amz-date:$amz_date
"
    local signed_headers="host;x-amz-content-sha256;x-amz-date"
    local canonical_request="$method
$canonical_uri

$canonical_headers
$signed_headers
$payload_hash"

    local scope="$date_stamp/$REGION/$SERVICE/aws4_request"
    local string_to_sign="AWS4-HMAC-SHA256
$amz_date
$scope
$(sha256_str "$canonical_request")"

    local k_secret_hex k_date k_region k_service k_signing signature
    k_secret_hex=$(printf '%s' "AWS4$SECRET_KEY" | xxd -p -c 256)
    k_date=$(hmac_hex "$k_secret_hex" "$date_stamp")
    k_region=$(hmac_hex "$k_date" "$REGION")
    k_service=$(hmac_hex "$k_region" "$SERVICE")
    k_signing=$(hmac_hex "$k_service" "aws4_request")
    signature=$(hmac_hex "$k_signing" "$string_to_sign")

    local auth="AWS4-HMAC-SHA256 Credential=$ACCESS_KEY/$scope, SignedHeaders=$signed_headers, Signature=$signature"

    local curl_args=(-sS --fail-with-body -X "$method"
        -H "Host: $HOST"
        -H "x-amz-date: $amz_date"
        -H "x-amz-content-sha256: $payload_hash"
        -H "Authorization: $auth"
        --connect-timeout 20 --max-time 900)
    [ "$method" = "PUT" ] && curl_args+=(--upload-file "$file")
    [ -n "$outfile" ] && curl_args+=(-o "$outfile")
    [ "$method" = "HEAD" ] && curl_args+=(-I)

    curl "${curl_args[@]}" "https://$HOST/$key"
}

case "${1:-}" in
--head)
    sign_and_send HEAD "$2" | grep -iE "^(HTTP/|content-length)" || true
    ;;
--get)
    sign_and_send GET "$2" "" "$3" >/dev/null
    echo "downloaded $2 -> $3"
    ;;
*)
    [ $# -eq 2 ] || { echo "usage: s3-put.sh <local-file> <remote-key>" >&2; exit 2; }
    [ -f "$1" ] || { echo "!! no such file: $1" >&2; exit 1; }
    sign_and_send PUT "$2" "$1" >/dev/null
    echo "uploaded $(basename "$1") -> s3://$BUCKET/$2"
    ;;
esac
