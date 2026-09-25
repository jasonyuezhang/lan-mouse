# Mac Mouse Fix integration: development status

## Current desk state (2026-09-19)

The standalone Mac Mouse Fix app remains removed. The user clarified that its
features should remain inside Lan Mouse, so the engine is now bundled privately
at `Lan Mouse Control.app/Contents/Library/MouseEngine.bundle`. Both Macs run its
helper through `de.feschber.lan-mouse.mouse-engine`, a per-user LaunchAgent managed
by the control panel. Its original standalone launch service stays disabled.
The embedded helper retains MMF's resources, version metadata, license checks,
identity and permissions, but presents no separate menu-bar icon. Embedded mode
skips the standalone-app removal watcher and prevents hiding the menu from
resetting the scroll/button switches. Opening the engine's main UI route opens
Lan Mouse instead. This is a private development build, not a combined public
release or a license change.

The native **Mouse** panel follows MMF’s Buttons / Scrolling layout. Its grouped
button table displays and edits click, double-click, hold, drag and scroll
assignments, with native effect menus, keyboard-shortcut recording, pointer-lock
options and confirmed 3-/5-button defaults. Add Action uses explicit button and
gesture selectors rather than MMF’s hover-to-record pad. Unknown effects and
compound triggers remain intact; stale edits are rejected if a peer changed the
row. Button 5’s existing two-finger pan appears as **Scroll & Navigate**; its
alternative is **Spaces & Mission Control**. Scrolling exposes smoothness,
trackpad simulation, direction, speed, precision and recorded keyboard modifiers.
Modifier assignments are unique, as in MMF. Shared preferences and engine
readiness remain integrated into Lan Mouse. **Keys & Buttons** remains the separate per-peer remapping editor.
Both `mouse-engine.enabled` flags and profile sync are enabled again. Both
embedded helpers passed capability and Accessibility checks. A live change to
Customize scrolling synchronized to the other Mac and was restored; hiding the
engine menu did not reset it. Existing settings are preserved.

Ethernet is still preferred. Self-assigned addresses changed after the original
setup; the active addresses are now `169.254.24.7` on this Mac and
`169.254.91.143` on the other Mac. Daemon logs show input handoffs over this link.
Wi-Fi remains a fallback. Self-assigned addresses can change again.

## Building the bundled engine

Build the modified sibling MMF checkout first with Xcode, then build the panel:

```sh
# From ../mac-mouse-fix:
xcodebuild -project 'Mouse Fix.xcodeproj' -scheme 'Helper - Release' -configuration Release -derivedDataPath ../.mmf-build CODE_SIGNING_ALLOWED=NO ARCHS=arm64 ONLY_ACTIVE_ARCH=YES build
# From this Lan Mouse checkout:
LAN_MOUSE_ENGINE_SIGN_IDENTITY='lan-mouse dev' macos-ui/build.sh
```

`LAN_MOUSE_ENGINE_BUNDLE` can override the source bundle path. The build embeds
the license notice and marks the private bundle `LanMouseEmbeddedEngine`.
Sign the engine with the appropriate development identity on each Mac so existing
Accessibility grants can remain valid. The host signature is then regenerated.
Do not replace a running embedded helper in place: stop its LaunchAgent first
when updating engine code, install the bundle, then reopen the control panel.
UI-only updates can replace the host executable without stopping the engine.

## Previously verified integration

The desired experience was one shared mouse profile, automatically synchronized
whenever paired Macs reconnect, without a physical mouse on the receiver.
Matching helper, daemon and control-panel builds were installed on both Macs.
Both helpers passed capability and Accessibility probes. The user confirmed
scrolling, extra buttons, Option+Up and crossing back worked. Live profile
convergence was verified in both directions before integration was disabled.

## Implemented and checked

- The daemon syncs the allowed profile over existing approved DTLS sessions,
  retries dropped chunks, reconciles logical revisions and preserves local
  license/identity/UI fields. Automated session tests exercise both directions,
  a dropped chunk, missing approval and revocation; these use an in-memory
  transport, not two real macOS input engines.
- Native Controls provides the shared profile and preserves the existing per-Mac
  mapping editor. Profile file edits coordinate with the daemon through flock.
  The panel reloads the installed helper through MMF's existing message port;
  the daemon's atomic edits are picked up by MMF's existing config watcher.
- Synthesized input carries CGEvent source-user-data `0x4c414e4d4f555345`.
  The tag identifies a virtual device; it is not a security credential.
- In the sibling checkout `../mac-mouse-fix`, the helper source recognizes a
  user-space remote Device and includes its capabilities when deciding which
  taps to enable. It retains existing permission, license and kill-switch gates.
  The modified helper builds successfully with Xcode 27. Both installed helpers
  pass permission checks, and basic physical input was verified by the user.

## Helper contract prepared in the MMF source

Send MMF's normal NSKeyedArchiver message dictionary to its existing local port
`com.nuebling.mac-mouse-fix.helper`, message ID `0x420666`:

```json
{"message":"lanMouseRemoteInput","payload":{"version":1,"active":true,"sending":false,"buttons":32}}
```

A supporting helper replies with `version: 1`, `accepted: true`, and `active`.
A supporting helper must also return `rawCapture: true`; an absent/unsupported
helper is reported as unavailable and ordinary input sharing continues.
Renew an active receiving session every two seconds, send `active: false` after
releasing input on leave, and stop renewal on disconnect. The helper removes the
virtual device after six seconds without renewal and cancels remote holds/drag
state. Physical devices continue to contribute capabilities normally.

The daemon now invokes this contract when input ownership changes and renews it
in the background. `sending: true` suspends source-side mouse actions; receiving
adds a virtual device. Both expire after six seconds without renewal. The helper
processes Session events and ignores its own generated events. Lan Mouse captures
at HID, before the helper, to forward raw physical input.

Create `~/.config/lan-mouse/mouse-engine.enabled` before starting the daemon to
opt into the modified helper. This is separate from preference synchronization.
Remove the old `mmf-toggle` enter/leave hooks only after installing and granting
permissions to the modified helper on both Macs. Existing hook backups are
retained for rollback. Pairing now offers shared preferences, seeded from the
inviting Mac, after the user confirms that sharing works.

## Historical installation and verification

- Full Xcode 27 build, Rust native workspace tests (72), clippy and native UI
  checks passed. Both daemons are running after installation.
- The modified helper is signed with each Mac's development identity. Both
  report the integration contract supported and Accessibility granted.
- Original apps, daemon and configuration are backed up under
  `~/.config/lan-mouse/backups/mmf-integration-20260919-152851` on the development
  Mac and `mmf-integration-20260919-153234` on the receiving Mac. Each backup also
  contains the previous control panel in `original-control-panel`.
- The development Mac's current profile was seeded at revision 1; the other
  Mac's current profile at revision 0, allowing the development Mac to seed
  the initial shared settings on connection. Each keeps its own node identity.
- Both machines have `mouse-engine.enabled` and enabled `mouse-profile.json`.
  A paired connection currently opens on the first screen crossing; profile
  replication starts with that connection and continues without the panel open.

Live verification on 2026-09-19:

- The user confirmed scrolling, extra buttons, Option+Up and crossing back work.
- Both Macs converged to revision 1 with identical profile hashes and installed
  configuration matching the profile. Republishing the same settings from the
  receiving Mac at revision 2 propagated back to the development Mac, confirming
  the reverse path without changing mouse behavior.
- Earlier connection attempts logged timeouts for the unplugged Thunderbolt path
  and transient Wi-Fi failures; later Wi-Fi sessions connected and carried input
  and profile updates successfully.

Extended manual checks remain: smooth scrolling variations, hold/drag actions,
rapid crossings, disconnect during a hold, recovery after daemon loss, and UI
edits with the other panel closed. Automated transport tests cover bidirectional
synchronization and dropped chunks, but do not replace these physical checks.

Development check commands (from the Lan Mouse checkout):

```sh
CARGO_TARGET_DIR=../.lan-mouse-target cargo test --workspace --exclude lan-mouse-gtk --no-default-features
CARGO_TARGET_DIR=../.lan-mouse-target cargo clippy --workspace --exclude lan-mouse-gtk --all-targets --no-default-features
macos-ui/test.sh
```

Full workspace/all-feature checks also build GTK and are currently blocked by
missing pkg-config/GTK dependencies on this Mac. This native macOS build does
not need GTK. The source checkout for MMF retains its own license and notices;
no combined distribution or license change has been made.

## Mouse panel validation (2026-09-20)

The Swift build and `macos-ui/test.sh` pass. Regression coverage includes native
pan/drag decoding, preserving unknown effects and local-only settings, rejecting
stale/duplicate rows, removal, and unique scroll modifiers. Both installed UI
binaries match after stripping their per-machine signatures from temporary
copies; both bundles pass signature verification. The two changed Swift sources
also match on both Macs.

A temporary Button 16 hold action was added through `MouseProfileModel.addAction`,
observed in the receiving Mac’s profile, removed through the same model, and its
removal observed remotely. Both profiles’ original Remaps, Scroll, Pointer and
General sections were verified restored. The live UI was inspected in Buttons
and Scrolling, including the selected Button 5 pan action, Add Action, recording
sheet cancellation and duplicate rejection. Physical dropdown/gesture interaction
is left for user verification; desktop automation did not reliably operate native
popup menus while sharing was active. Helpers and sharing stayed running during
UI installation.

## Button-drag zoom (2026-09-20)

The custom helper now supports `dragTrigger` with `modifiedDragType: zoom`.
The Mouse panel lists **Zoom In or Out** alongside its pan and Spaces drag
actions. Button 4 + drag is configured in addition to Button 4 + scroll zoom.
Drag up to enlarge, down to shrink; release ends the gesture. Horizontal movement
is ignored. Direction is independent of the macOS natural-scroll preference.
The engine reuses scroll zoom’s magnification output and Chromium compatibility;
its normal drag threshold, click suppression, pointer-lock option and input-release
path still apply. Both computers need this modified helper and the updated
profile validator before syncing this action.

Validation: Xcode Release build, Swift UI build/tests, 32 Rust library tests,
`cargo fmt --check`, and native library clippy passed.
`../mac-mouse-fix/Tests/test_drag_zoom.py` compiles the actual plugin and shared
zoom-output methods against event-posting doubles, checking direction, horizontal
input, begin/change/end/cancel, pointer unfreezing and Chromium startup behavior.
It posts no input to the desktop. Physical feel and target-app zoom behavior
require a mouse test on each Mac.

Installed and verified on both Macs: signed UI, helper and daemon builds match
(after removing per-machine signatures from temporary executable copies). Both
helpers retain Accessibility permission. The shared profile converged at revision
7 with matching documents and six assignments, including Button 4 + drag zoom
and its original scroll zoom. The user confirmed that drag zoom works on both
Macs and stops on release. Engine/UI and sharing-service rollback copies are in
`~/.config/lan-mouse/backups/drag-zoom-*` and `drag-zoom-sync-*`. The receiving Mac
also has the engine source update archive alongside its engine backup; the full
MMF source checkout remains on the development Mac at `~/src/mac-mouse-fix`.
