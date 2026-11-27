#!/usr/bin/env bash
# Signs (Developer ID, hardened runtime, secure timestamp) and notarizes the macOS marklark binaries,
# in place: the release archives (and their .sha256 files) and the wheels.
#
#   .github/scripts/macos-sign.sh <archives-dir> <wheels-dir>
#
# Environment:
#   APPLE_CERT_SHA1      the certificate to sign with, by SHA-1 hash (never by name: the name has an
#                        accented letter that codesign mishandles)
#   APPLE_CERT_P12       base64 .p12 with that certificate and its key; imported into a temporary
#                        keychain. Without it, the login keychain is used (local testing).
#   APPLE_CERT_PASSWORD  the .p12 password
#   APPLE_API_KEY_P8     base64 App Store Connect API key (.p8) for notarytool
#   APPLE_API_KEY_ID, APPLE_API_ISSUER_ID
#   NOTARIZE=0           sign only (local testing)
#
# A bare Mach-O binary cannot carry a stapled ticket; Gatekeeper looks the notarization up online.
set -euo pipefail

archives=$1
wheels=$2
: "${APPLE_CERT_SHA1:?APPLE_CERT_SHA1 is required}"
# As copied from Keychain Access the hash has spaces and may be lower case.
APPLE_CERT_SHA1=$(printf '%s' "$APPLE_CERT_SHA1" | tr -d ' \t\r\n:' | tr '[:lower:]' '[:upper:]')
work=$(mktemp -d)
cleanup() {
    if [ -n "${keychain:-}" ]; then security delete-keychain "$keychain" 2>/dev/null || true; fi
    rm -rf "$work"
}
trap cleanup EXIT

keychain_args=()
if [ -n "${APPLE_CERT_P12:-}" ]; then
    keychain="$work/signing.keychain-db"
    kc_password=$(uuidgen)
    security create-keychain -p "$kc_password" "$keychain"
    security set-keychain-settings -lut 3600 "$keychain"
    security unlock-keychain -p "$kc_password" "$keychain"
    printf '%s' "$APPLE_CERT_P12" | tr -d ' \r\n' | base64 --decode > "$work/cert.p12"
    security import "$work/cert.p12" -k "$keychain" -P "${APPLE_CERT_PASSWORD:-}" -T /usr/bin/codesign
    rm "$work/cert.p12"
    security set-key-partition-list -S apple-tool:,apple: -s -k "$kc_password" "$keychain" > /dev/null
    # codesign needs the keychain in the search list to build the certificate chain.
    security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')
    # The Developer ID G2 intermediate: runner images do not always have it, and without the chain
    # codesign reports "no identity found".
    curl -fsSL https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer -o "$work/g2.cer"
    security import "$work/g2.cer" -k "$keychain" > /dev/null 2>&1 || true
    keychain_args=(--keychain "$keychain")
    echo "signing identities in the temporary keychain:"
    security find-identity -v -p codesigning "$keychain"
    if ! security find-identity -v -p codesigning "$keychain" | grep -q "$APPLE_CERT_SHA1"; then
        echo "APPLE_CERT_SHA1 does not match the certificate in APPLE_CERT_P12 (listed above):" >&2
        echo "the hash and the .p12 must be the same certificate." >&2
        exit 1
    fi
fi

sign() {
    codesign --force --sign "$APPLE_CERT_SHA1" ${keychain_args[@]+"${keychain_args[@]}"} --options runtime --timestamp "$1"
    codesign --verify --strict --verbose=2 "$1"
}

signed=()   # every signed binary, for one notarization submission

# Release archives: marklark-<version>-<target>.tar.gz holding marklark-<version>-<target>/marklark.
for archive in "$archives"/marklark-*-apple-darwin.tar.gz; do
    [ -e "$archive" ] || continue
    name=$(basename "$archive" .tar.gz)
    mkdir -p "$work/a"
    tar xzf "$archive" -C "$work/a"
    sign "$work/a/$name/marklark"
    signed+=("$work/a/$name/marklark")
done

# Wheels: unpack, sign the binary, repack (wheel pack rewrites RECORD with the new hashes).
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install --quiet wheel
for whl in "$wheels"/*macosx*.whl; do
    [ -e "$whl" ] || continue
    dir="$work/w/$(basename "$whl" .whl)"
    mkdir -p "$dir"
    "$work/venv/bin/wheel" unpack "$whl" -d "$dir"
    bin=$(find "$dir" -path '*.data/scripts/marklark' -type f)
    sign "$bin"
    signed+=("$bin")
done

if [ "${#signed[@]}" -eq 0 ]; then
    echo "no macOS binaries found" >&2
    exit 1
fi

if [ "${NOTARIZE:-1}" != 0 ]; then
    printf '%s' "$APPLE_API_KEY_P8" | tr -d ' \r\n' | base64 --decode > "$work/key.p8"
    mkdir "$work/submit"
    i=0
    for b in "${signed[@]}"; do i=$((i + 1)); mkdir "$work/submit/$i"; cp "$b" "$work/submit/$i/"; done
    ditto -c -k "$work/submit" "$work/submit.zip"
    output=$(xcrun notarytool submit "$work/submit.zip" --key "$work/key.p8" \
        --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" --wait 2>&1) || true
    printf '%s\n' "$output"
    submission=$(printf '%s\n' "$output" | awk '/^ *id: /{print $2; exit}')
    status=$(printf '%s\n' "$output" | awk '/^ *status: /{print $2; exit}')
    if [ "$status" != Accepted ]; then
        [ -n "$submission" ] && xcrun notarytool log "$submission" --key "$work/key.p8" \
            --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" || true
        echo "notarization was ${status:-not completed}" >&2
        exit 1
    fi
fi

# Write the signed binaries back.
for archive in "$archives"/marklark-*-apple-darwin.tar.gz; do
    [ -e "$archive" ] || continue
    name=$(basename "$archive" .tar.gz)
    tar czf "$archive" -C "$work/a" "$name"
    (cd "$archives" && shasum -a 256 "$(basename "$archive")" > "$(basename "$archive").sha256")
done
for whl in "$wheels"/*macosx*.whl; do
    [ -e "$whl" ] || continue
    dir="$work/w/$(basename "$whl" .whl)"
    rm "$whl"
    "$work/venv/bin/wheel" pack "$dir"/* -d "$wheels"
    [ -e "$whl" ] || { echo "wheel pack produced a different name than $(basename "$whl")" >&2; exit 1; }
done
echo "signed ${#signed[@]} binaries"
