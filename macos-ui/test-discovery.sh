#!/bin/sh
# Publishes two short-lived test advertisements; grants no input access.
set -eu
cd "$(dirname "$0")"
test_binary=$(mktemp -t lan-mouse-discovery-tests)
trap 'rm -f "$test_binary"' EXIT
xcrun swiftc -swift-version 5 -parse-as-library Discovery.swift DiscoveryLiveTests.swift -o "$test_binary"
"$test_binary"
