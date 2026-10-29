#!/bin/sh
# Builds EvoParseq.app for the host Mac and zips it into dist/.
# Ad-hoc signed only: other Macs need "Open Anyway" in Privacy & Security
# until a Developer ID signature and notarization are added.
set -eu
cd "$(dirname "$0")/.."

version=$(tr -d '[:space:]' < VERSION)
manifest_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' src-native/Cargo.toml | head -n 1)
[ "$manifest_version" = "$version" ] || { echo "Native version mismatch" >&2; exit 1; }

target=$(rustc -vV | sed -n 's/^host: //p')
cargo build --locked --release --manifest-path src-native/Cargo.toml --target "$target"
binary="src-native/target/$target/release/evoparseq"
arch=$(lipo -archs "$binary")

staging=$(mktemp -d "${TMPDIR:-/tmp}/evoparseq-package.XXXXXX")
trap 'rm -rf "$staging"' EXIT
package_name="EvoParseq-v$version-macos-$arch"
package="$staging/$package_name"
app="$package/EvoParseq.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/EvoParseq"

iconset="$staging/AppIcon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
    sips -s format png -z "$size" "$size" assets/evoparseq.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -s format png -z "$double" "$double" assets/evoparseq.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleExecutable</key><string>EvoParseq</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIdentifier</key><string>io.github.evoparseq</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>EvoParseq</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
codesign --force --sign - --timestamp=none "$app"
codesign --verify --strict "$app"
"$app/Contents/MacOS/EvoParseq" --version

cp README.md LICENSE CHANGELOG.md docs/macos.md "$package/"
cp src-native/vendor/webrtc-0.14.0/LICENSE-MIT "$package/WEBRTC-LICENSE-MIT.txt"
cp src-native/vendor/webrtc-0.14.0/LICENSE-APACHE "$package/WEBRTC-LICENSE-APACHE.txt"
cp src-native/vendor/dtls-0.13.0/LICENSE-MIT "$package/DTLS-LICENSE-MIT.txt"
cp src-native/vendor/dtls-0.13.0/LICENSE-APACHE "$package/DTLS-LICENSE-APACHE.txt"
cp src-native/vendor/dtls-0.13.0/EVOPARSEQ.md "$package/DTLS-LOCAL-CHANGES.md"
cp src-native/vendor/webrtc-0.14.0/LOCAL-CHANGES.md "$package/WEBRTC-LOCAL-CHANGES.md"
cp src-native/vendor/webrtc-ice-0.14.0/LOCAL-CHANGES.md "$package/ICE-LOCAL-CHANGES.md"
cp src-native/vendor/turn-0.11.0/LOCAL-CHANGES.md "$package/TURN-LOCAL-CHANGES.md"
cp src-native/crates/bud/vendor/lowlat-client/LICENSE "$package/LOWLAT-LICENSE.txt"
cp src-native/crates/bud/vendor/lowlat-client/EVOPARSEQ.md "$package/BUD-LOCAL-CHANGES.md"
cp src-native/crates/bud/vendor/lowlat-net/EVOPARSEQ.md "$package/BUD-NET-LOCAL-CHANGES.md"
opus=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/libopus_sys-0.3.3 | head -n 1)
[ -d "$opus" ] || { echo "Bundled Opus license directory missing" >&2; exit 1; }
cp "$opus/LICENSE" "$package/LIBOPUS-SYS-LICENSE.txt"
cp "$opus/opus/COPYING" "$package/OPUS-LICENSE.txt"

mkdir -p dist
zip="dist/$package_name.zip"
rm -f "$zip"
ditto -c -k --norsrc --noextattr --keepParent "$package" "$zip"
shasum -a 256 "$zip" | sed 's| dist/| |' > "dist/SHA256SUMS-macos.txt"
echo "$zip"
