#!/bin/sh
set -eu
cd "$(dirname "$0")"
test_binary=$(mktemp -t lan-mouse-ui-tests)
trap 'rm -f "$test_binary"' EXIT
xcrun swiftc -swift-version 5 -parse-as-library ControlModel.swift Discovery.swift MouseProfile.swift SharingShortcut.swift ChromeHandoff.swift FileBridge.swift CoreTests.swift -o "$test_binary"
"$test_binary"
xcrun swiftc -swift-version 5 -parse-as-library LitraGlow.swift LitraGlowTests.swift -o "$test_binary"
"$test_binary"
xcrun swiftc -swift-version 5 -parse-as-library SecureInput.swift SecureInputTests.swift -o "$test_binary"
"$test_binary"
