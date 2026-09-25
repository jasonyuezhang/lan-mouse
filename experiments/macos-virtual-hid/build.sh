#!/bin/sh
set -eu
# Supply a checkout of pqrs-org/Karabiner-DriverKit-VirtualHIDDevice v8.0.0.
[ "$#" -eq 2 ] || { echo "Usage: $0 /path/to/driver-v8.0.0 /path/to/output-directory"; exit 2; }
driver=$1
output=$2
here=$(python3 -c 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).resolve().parent)' "$0")
python3 - "$driver/version.json" <<'PY'
import json, sys
v=json.load(open(sys.argv[1]))
assert v == {'package_version':'8.0.0','driver_version':'1.8.0','client_protocol_version':7}, v
PY
mkdir -p "$output"
clang++ -std=c++23 -O2 -Wall -Wextra -Werror -pthread \
  -isystem "$driver/include" -isystem "$driver/vendor/vendor/include" \
  "$here/probe.cpp" -o "$output/virtual-hid-probe"
cp "$here/run.sh" "$output/run.sh"
cp "$here/README.md" "$output/README.md"
