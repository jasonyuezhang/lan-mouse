#!/usr/bin/env python3
"""Run the native cursor regression check, restoring the sharing service afterwards."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--driver', required=True)
parser.add_argument('--identity', required=True)
parser.add_argument('--keychain', required=True)
args = parser.parse_args()
root = Path(tempfile.mkdtemp(prefix='lan-mouse-cursor-check-'))
driver = root / 'cursor-check'
shutil.copy2(args.driver, driver)
subprocess.run(['codesign', '--force', '--sign', args.identity, '--keychain', args.keychain,
                '--identifier', 'de.feschber.lan-mouse', '--timestamp=none', str(driver)], check=True)
service = f'gui/{os.getuid()}/de.feschber.lan-mouse'
plist = Path.home() / 'Library/LaunchAgents/de.feschber.lan-mouse.plist'
installed = str(Path.home() / '.local/bin/lan-mouse')
was_loaded = subprocess.run(['launchctl', 'print', service], capture_output=True).returncode == 0
stopped = False
try:
    if was_loaded:
        subprocess.run(['launchctl', 'bootout', service], check=True)
        stopped = True
    # bootout can return before the old daemon exits and releases its cursor.
    deadline = time.monotonic() + 10
    while True:
        processes = subprocess.check_output(['ps', '-axo', 'comm='], text=True).splitlines()
        if installed not in (line.strip() for line in processes):
            break
        if time.monotonic() > deadline:
            raise RuntimeError('installed daemon is still running; refusing to overlap capture')
        time.sleep(.05)
    with (root / 'result.log').open('w') as log:
        result = subprocess.run([str(driver)], stdout=log, stderr=subprocess.STDOUT, timeout=25)
    print((root / 'result.log').read_text())
    result.check_returncode()
finally:
    if stopped:
        subprocess.run(['launchctl', 'bootstrap', f'gui/{os.getuid()}', str(plist)], check=True)
    print('Native cursor test artifacts:', root)
