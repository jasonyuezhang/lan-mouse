#!/usr/bin/env python3
"""Exercise native drag delivery in temporary apps using production Rust emulation.
Build the example and NativeDragSmoke first; pass their paths and a signing identity.
Temporarily pauses the local sharing service, then restores its prior state.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--driver', required=True)
p.add_argument('--app', required=True)
p.add_argument('--identity', required=True)
p.add_argument('--keychain', required=True)
p.add_argument('--cancel', action='store_true')
a = p.parse_args()
root = Path(tempfile.mkdtemp(prefix='lan-mouse-native-drag-'))
(root / 'fixture.txt').write_text('Lan Mouse native drag test\n')
binary = root / 'driver'
shutil.copy2(a.driver, binary)
subprocess.run(['codesign', '--force', '--sign', a.identity, '--keychain', a.keychain,
                '--identifier', 'de.feschber.lan-mouse', '--timestamp=none', str(binary)], check=True)
env = dict(os.environ, LAN_MOUSE_NATIVE_DRAG_TEST_DIR=str(root))
service = f'gui/{os.getuid()}/de.feschber.lan-mouse'
plist = Path.home() / 'Library/LaunchAgents/de.feschber.lan-mouse.plist'
was_loaded = subprocess.run(['launchctl', 'print', service], capture_output=True).returncode == 0
procs = []
stopped = False
try:
    if was_loaded:
        subprocess.run(['launchctl', 'bootout', service], check=True)
        stopped = True
    for mode in ['target', 'receiver']:
        with (root / (mode + '.log')).open('w') as log:
            procs.append(subprocess.Popen([a.app, mode], env=env, stdout=log, stderr=subprocess.STDOUT))
    deadline = time.monotonic() + 5
    while not ((root / 'target.json').exists() and (root / 'receiver-ready').exists()):
        if time.monotonic() > deadline:
            raise RuntimeError('test windows not ready')
        time.sleep(.05)
    subprocess.run([str(binary)] + (['--cancel'] if a.cancel else []), env=env, timeout=12, check=True)
finally:
    for proc in procs:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
    if stopped:
        subprocess.run(['launchctl', 'bootstrap', f'gui/{os.getuid()}', str(plist)], check=True)
    print('Native test artifacts:', root)
    if (root / 'receiver.log').exists():
        print((root / 'receiver.log').read_text())
