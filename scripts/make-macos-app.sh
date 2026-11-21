#!/bin/sh
# Builds target/release/Tagcast Lite.app from the release binary.
# Set CODESIGN_IDENTITY to sign with a Developer ID instead of ad hoc.
set -eu

cd "$(dirname "$0")/.."
cargo build --locked --release

build_dir="${CARGO_TARGET_DIR:-target}"
if [ -n "${CARGO_BUILD_TARGET:-}" ]; then
    build_dir="$build_dir/$CARGO_BUILD_TARGET"
fi
app="$build_dir/release/Tagcast Lite.app"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$build_dir/release/tagcast-player" "$app/Contents/MacOS/tagcast-player"
cp LICENSE README.md assets/fonts/Inter-LICENSE.txt "$app/Contents/Resources/"

iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" assets/icon-1024.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" assets/icon-1024.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Tagcast Lite</string>
    <key>CFBundleDisplayName</key><string>Tagcast Lite</string>
    <key>CFBundleIdentifier</key><string>nl.cyberdelia.tagcast-player</string>
    <key>CFBundleExecutable</key><string>tagcast-player</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

identity=${CODESIGN_IDENTITY:--}
if [ "$identity" = "-" ]; then
    codesign --force --sign - "$app"
else
    codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
echo "Built $app"
