#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -m)" == arm64 ]] || { echo "需要 Apple Silicon"; exit 1; }
export MACOSX_DEPLOYMENT_TARGET=15.0
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/.build/target}"
mkdir -p .build dist
stage=$(mktemp -d "$PWD/.build/package.XXXXXX")
trap 'rm -rf "$stage"' EXIT
version=0.10.0
core_version=1.19.33
core_digest=060b64b0f2c9a310831db0fe4adb2878de67bfa606c311813c1f13966aa504b6
core_archive="$PWD/.build/mihomo.gz"
if [[ ! -f "$core_archive" ]]; then
  curl -fLsS --retry 2 "https://github.com/MetaCubeX/mihomo/releases/download/v$core_version/mihomo-darwin-arm64-v$core_version.gz" -o "$stage/core.gz"
  mv "$stage/core.gz" "$core_archive"
fi
echo "$core_digest  $core_archive" | shasum -a 256 -c -
cargo build --release --locked --manifest-path src-tauri/Cargo.toml
app="$stage/Profile Harbor.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$stage/AppIcon.iconset"
cp "$CARGO_TARGET_DIR/release/profile-harbor" "$app/Contents/MacOS/"
xcrun clang -arch arm64 -mmacosx-version-min=15.0 -Os -Wall -Wextra -Werror native/core-guard.c -o "$app/Contents/MacOS/profile-harbor-guard"
xcrun swiftc -O -target arm64-apple-macosx15.0 -module-cache-path "$PWD/.build/swift-cache" native/ObscuraConnection.swift native/ObscuraViewer.swift -o "$app/Contents/MacOS/profile-harbor-viewer"
gzip -dc "$core_archive" > "$app/Contents/MacOS/mihomo"
chmod +x "$app/Contents/MacOS/mihomo"
cp -R resources/licenses "$app/Contents/Resources/"
for n in 16 32 128 256 512; do
  sips -z "$n" "$n" src-tauri/icons/icon.png --out "$stage/AppIcon.iconset/icon_${n}x${n}.png" >/dev/null
  sips -z "$((n*2))" "$((n*2))" src-tauri/icons/icon.png --out "$stage/AppIcon.iconset/icon_${n}x${n}@2x.png" >/dev/null
done
iconutil -c icns "$stage/AppIcon.iconset" -o "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>profile-harbor</string>
<key>CFBundleIdentifier</key><string>dev.provencejake.profileharbor</string>
<key>CFBundleName</key><string>Profile Harbor</string>
<key>CFBundleDisplayName</key><string>Profile Harbor</string>
<key>CFBundleVersion</key><string>1000</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
identity="${MACOS_SIGN_IDENTITY:--}"
codesign --force --options runtime --sign "$identity" "$app/Contents/MacOS/profile-harbor-guard"
codesign --force --options runtime --sign "$identity" "$app/Contents/MacOS/profile-harbor-viewer"
codesign --force --options runtime --sign "$identity" "$app/Contents/MacOS/mihomo"
codesign --force --options runtime --sign "$identity" "$app"
codesign --verify --deep --strict "$app"
if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  ditto -c -k --keepParent "$app" "$stage/notarize.zip"
  xcrun notarytool submit "$stage/notarize.zip" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$app"
fi
ditto -c -k --keepParent "$app" "$stage/release.zip"
mv "$stage/release.zip" "dist/Profile-Harbor-$version-mac-arm64.zip"
ditto "$app" "dist/Profile Harbor.app"
