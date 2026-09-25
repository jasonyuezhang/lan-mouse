# Universal Control virtual-HID experiment

This tests whether the receiving Mac can forward Karabiner virtual-device input
to an iPad via Universal Control. It is **not** a Lan Mouse backend, and does not
prove that the full Mac → Lan Mouse → Mac → iPad chain works. Run on the Mac
already paired with the iPad; first verify its physical trackpad/keyboard works.

The installed Karabiner package on that Mac is 8.0.0 (driver 1.8.0, client
protocol 7). The helper uses its public C++ client library, pinned to upstream
`v8.0.0`; no new driver, entitlement, or persistent service is installed.
Karabiner's daemon and client require root, so run the commands in Terminal on
that Mac. Do not share your password with the agent.

## Run

The prepared copy lives at `~/lan-mouse-hid-test` on the receiving Mac.

1. `sudo ~/lan-mouse-hid-test/run.sh check`
   Expect `READY: virtual keyboard and pointing device`. This mode sends no
   nonzero input reports. A readiness failure means the experiment is not ready;
   it says nothing about Universal Control compatibility.
2. Open a blank TextEdit document on the Mac. Run:
   `sudo ~/lan-mouse-hid-test/run.sh keyboard`
   During the eight-second countdown, focus the blank document. It should receive
   `hidtest` once, with no Return. This is the virtual keyboard baseline.
3. Open a blank note on the iPad. Run the same keyboard command, then use the Mac's
   physical trackpad to focus that note during the countdown. Does `hidtest`
   appear on the iPad? Text on the Mac instead is not an iPad success.
4. Put the pointer on the Mac just inside the edge that leads to the iPad.
   Run `sudo ~/lan-mouse-hid-test/run.sh left` (or `right`, `up`, `down` for the
   iPad's actual location). Reposition the pointer during the countdown.
   The test moves 400 relative HID units over two seconds, with no clicks.
   It must visibly move on the Mac before an edge-crossing failure is meaningful.
   If it stops at the edge, confirm physical crossing still works at that same
   position; the bounded movement may require starting closer to the boundary.

Each invocation pauses only the receiving Mac's Lan Mouse LaunchAgent and
restores it when the test exits, including Ctrl-C. Use that Mac's own keyboard
and trackpad while it is paused. Existing Karabiner daemons are left running;
a daemon started by this wrapper is stopped at exit. Reports are released and
the helper's virtual devices destroyed on normal completion or Ctrl-C. A forced
kill/power failure cannot run the shell cleanup; restart Lan Mouse with:

```
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/de.feschber.lan-mouse.plist
```

`Reports submitted` means only that the client queued reports. It does **not**
assert delivery to an application or iPad. Record each result independently:
Mac typing, iPad typing after physical focus, and pointer crossing from the Mac.
Passing all three justifies a separate minimal Lan Mouse backend experiment;
partial results identify which path Universal Control accepts.

## Build and validation

From the Lan Mouse repository root:

```
experiments/macos-virtual-hid/build.sh /path/to/Karabiner-DriverKit-VirtualHIDDevice-v8.0.0 /tmp/lan-mouse-hid-test
/tmp/lan-mouse-hid-test/virtual-hid-probe keyboard --dry-run
bash -n experiments/macos-virtual-hid/run.sh
```

The SDK source is https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/tree/v8.0.0.
It includes its third-party headers, so only Apple's command-line compiler is
needed. `--dry-run` validates the selected mode and prints its plan without
connecting to the daemon or creating devices; it is not a hardware test.
