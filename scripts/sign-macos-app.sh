#!/usr/bin/env bash
# Sign and notarize a built app on a macOS release runner.
set -euo pipefail

app=${1:?Pass the path to the built macOS app}
test -d "$app"
: "${CSC_LINK:?Set CSC_LINK to the base64-encoded Developer ID PKCS#12}"
: "${CSC_KEY_PASSWORD:?Set CSC_KEY_PASSWORD to the PKCS#12 password}"
: "${APPLE_ID:?Set APPLE_ID to the notarization Apple Account}"
: "${APPLE_APP_SPECIFIC_PASSWORD:?Set APPLE_APP_SPECIFIC_PASSWORD}"
: "${APPLE_TEAM_ID:?Set APPLE_TEAM_ID for the Developer ID certificate}"

work=$(mktemp -d)
keychain="$work/signing.keychain-db"
trap 'security delete-keychain "$keychain" >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
password=$(openssl rand -hex 32)
printf '%s' "${CSC_LINK#*;base64,}" | /usr/bin/base64 -D > "$work/certificate.p12"
security create-keychain -p "$password" "$keychain"
security unlock-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 1800 "$keychain"
security import "$work/certificate.p12" -k "$keychain" -P "$CSC_KEY_PASSWORD" -T /usr/bin/codesign
rm "$work/certificate.p12"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain"

identity=$(security find-identity -v -p codesigning "$keychain" | awk -v team="$APPLE_TEAM_ID" '
    /"Developer ID Application:/ && index($0, "(" team ")") { print $2; count++ }
    END { if (count != 1) exit 1 }
')
codesign --force --options runtime --timestamp --keychain "$keychain" --sign "$identity" "$app"
codesign --verify --strict "$app"

ditto -c -k --keepParent "$app" "$work/app.zip"
if ! xcrun notarytool submit "$work/app.zip" \
    --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait --timeout 12m --output-format json > "$work/notarization.json" ||
    ! jq -e '.status == "Accepted"' "$work/notarization.json" >/dev/null
then
    cat "$work/notarization.json" >&2
    exit 1
fi
xcrun stapler staple "$app"
xcrun stapler validate "$app"
spctl --assess --type execute "$app"
