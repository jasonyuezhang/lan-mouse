#!/bin/sh
set -eu
cd "$(dirname "$0")"
out="${1:-$HOME/Applications/Lan Mouse Control.app}"
# Keep the installed app's identity across rebuilds so macOS can retain grants.
app_signing_identity="${LAN_MOUSE_APP_SIGN_IDENTITY:-}"
if [ -z "$app_signing_identity" ] && [ -d "$out" ]; then
    app_signing_identity=$(codesign -d --verbose=2 "$out" 2>&1 | sed -n 's/^Authority=//p' | head -n 1)
fi
app_signing_identity="${app_signing_identity:-${LAN_MOUSE_ENGINE_SIGN_IDENTITY:--}}"
mkdir -p "$out/Contents/MacOS" "$out/Contents/Resources"
xcrun swiftc -swift-version 5 -O -parse-as-library -target arm64-apple-macosx14.0 ControlModel.swift Discovery.swift Pairing.swift MouseProfile.swift SharingShortcut.swift ChromeHandoff.swift FileBridge.swift MousePanel.swift LanMouseControl.swift -o "$out/Contents/MacOS/LanMouseControl"
daemon="../.lan-mouse-target/release/lan-mouse"
[ -x "$daemon" ] || daemon="../../.lan-mouse-target/release/lan-mouse"
[ -x "$daemon" ] || { echo "Build the daemon with cargo build --release --no-default-features first" >&2; exit 1; }
cp "$daemon" "$out/Contents/Resources/lan-mouse-daemon"
cp Keys.json "$out/Contents/Resources/Keys.json"
cat > "$out/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>LanMouseControl</string>
<key>CFBundleIdentifier</key><string>de.feschber.lan-mouse.control</string>
<key>CFBundleName</key><string>Lan Mouse Control</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSLocalNetworkUsageDescription</key><string>Find nearby Macs for Lan Mouse pairing.</string>
<key>NSBonjourServices</key><array><string>_lanmouse._udp</string></array>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
iconset=$(mktemp -d -t lan-mouse-icon).iconset
mkdir -p "$iconset"
trap 'rm -rf "$iconset" "${iconset%.iconset}"' EXIT
xcrun swift Icon.swift "$iconset"
iconutil -c icns "$iconset" -o "$out/Contents/Resources/AppIcon.icns"
# Keep MMF's resources/version metadata private inside the host; only its helper
# runs. No standalone app or second menu-bar item is installed.
engine="${LAN_MOUSE_ENGINE_BUNDLE:-../../.mmf-build/Build/Products/Release/Mac Mouse Fix.app}"
[ -d "$engine" ] || { echo "Build the integrated MMF helper first (see MMF-INTEGRATION.md)" >&2; exit 1; }
embedded="$out/Contents/Library/MouseEngine.bundle"
mkdir -p "$out/Contents/Library"
ditto "$engine" "$embedded"
/usr/libexec/PlistBuddy -c 'Add :LanMouseEmbeddedEngine bool true' "$embedded/Contents/Info.plist" 2>/dev/null || /usr/libexec/PlistBuddy -c 'Set :LanMouseEmbeddedEngine true' "$embedded/Contents/Info.plist"
cp ../../mac-mouse-fix/License "$embedded/Contents/Resources/MMF-License.txt"
codesign --force --deep --sign "${LAN_MOUSE_ENGINE_SIGN_IDENTITY:--}" --timestamp=none "$embedded"
codesign --force --sign "$app_signing_identity" --timestamp=none "$out"
echo "$out"
