#!/bin/sh
set -eu
test_binary=$(mktemp -t lan-mouse-litra-tests)
trap 'rm -f "$test_binary"' EXIT
xcrun swiftc -swift-version 5 -parse-as-library macos-ui/LitraGlow.swift macos-ui/LitraGlowTests.swift -o "$test_binary"
"$test_binary" "$@"
