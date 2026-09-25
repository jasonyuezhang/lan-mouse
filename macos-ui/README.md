# Lan Mouse Control for macOS

A native SwiftUI companion for the Lan Mouse daemon. Requires an Apple silicon
Mac running macOS 14 or newer. It uses the existing local JSON IPC socket; it
opens no web server and requires no third-party UI packages.

## Build and open

From the repository root:

```sh
cargo build --release --no-default-features
macos-ui/build.sh
open "$HOME/Applications/Lan Mouse Control.app"
```

The build script uses the repository's `.lan-mouse-target` release directory and
bundles that daemon. Build the daemon first, especially after an IPC change.
The app is locally signed, not a notarized distribution package. Existing
installations retain their separately signed daemon and macOS permissions;
upgrading the app does **not** replace an installed daemon. Install the matching
daemon build separately when upgrading (for this development setup,
`~/.local/bin/lan-mouse-install`). The mapping editor requires the daemon's
`UpdateKeyMap` IPC request added with this panel.

Run `macos-ui/test.sh` for state-decoding, identity association, updated discovery records, fingerprints, and latency parser checks.
Run `macos-ui/test-discovery.sh` for live Bonjour invitation/progress/cancellation updates; it publishes two temporary test advertisements without authorizing input.

## Sharing shortcut

Choose **Sharing Shortcut…** in the menu bar or on Home, click **Record Shortcut…**,
and press your preferred key combination. Include Command, Control, or Option;
Shift is optional. Save applies it, Remove Shortcut clears it, and Cancel preserves
the previous setting. The four-modifier emergency-release chord is reserved.
Unavailable shortcuts are reported without replacing your saved choice.

The shortcut toggles this Mac’s sharing service from any app, including while
controlling the other Mac. Keep Lan Mouse Control running (its window can be
closed). Configure it on each Mac whose keyboard you use; shortcuts are local
because other apps’ shortcut assignments can differ. No shortcut is assigned by
default. While forwarding, the daemon consumes the shortcut and releases held
keys before notifying the control panel. A ten-second renewed lease prevents a
quit/crashed panel from reserving the shortcut indefinitely.

## Drag a Chrome tab to the other Mac

With **Drag files between Macs** on at both Macs, pull a tab out of its Chrome
window (or drag a one-tab window by its title bar, or drag a link or the address
bar) toward the paired edge. When the card says Ready, keep going: the page opens
in Chrome on the other Mac, in the profile with the same name (for example
"Nash" or "Jason"). Escape is sent to Chrome here, so the dragged tab returns to
where it was. Only `http`/`https` pages cross, and only from Chrome.

- The profile is read from Chrome's window title, which ends in
  `Google Chrome - Person (Profile)` once Chrome has several profiles. Reading it,
  and spotting a pulled-out tab, needs Accessibility access for Lan Mouse Control
  on the sending Mac (the settings card offers a button).
- The receiver maps the name to a profile folder through Chrome's `Local State`.
  If macOS privacy settings block that file, it brings forward an open window of
  that profile first, since Chrome opens links in its last-active profile.
  Without such a window the page opens in the last-used profile.
- Dragging a multi-tab window, resizing a window, and drags inside a window are
  unaffected.

The page travels as a small `.lanmouse-url` file over the same encrypted
channel, from a paired Mac only.

## Drag a file into Slack or Orca on another Mac

Enable **Home → Drag files between Macs** on both Macs and keep both control
panels running. Drag one regular file (up to 64 MB) toward the paired screen edge.
Copying starts while approaching within 240 points of the edge (or while holding
within its final 30 points). Drags elsewhere do not start a transfer. If Copying
is still showing at the edge, keep holding; when it says Ready,
continue across the edge and drop into the destination app. The bridge offers a
native file URL, with no app allowlist or destination-specific adapter.

- **Slack:** drop into the message field of a channel or DM. Check the attachment
  preview, then send the message yourself. For a check without messaging anyone,
  use your self-DM and remove the draft attachment afterwards.
- **Orca:** drop into the agent's chat composer so it can reference the local copy.
- Other apps may accept the same native file drop through their normal drop areas.

No message or agent prompt is submitted automatically. Slack's own file and
workspace restrictions still apply; this bridge's limit remains one file up to
64 MB. See [Slack's attachment instructions](https://slack.com/help/articles/201330736-Add-files-to-Slack).
The copy channel has been checked in both directions, and a physical drop into
Orca has been confirmed. Slack's physical native-drop check is still pending.

Transfers use a bounded window on the existing authenticated, encrypted Lan Mouse
connection. No SMB share, SSH login, or extra listening port is required. The source
file is unchanged. A SHA-256 check completes before the remote drag is activated.
Received copies remain at `~/.config/lan-mouse/file-bridge/received/<transfer-id>/files/`
so agent references survive disconnection. These files are retained until manually
removed; removing one can break an existing agent reference. Cancelling before
crossing never inserts anything into the destination app. Folders and multiple simultaneous files
are currently rejected with an explanation.

The macOS capture backend only lets an existing left-button drag cross while the
control panel renews its 1.5-second ready lease. Ordinary window/selection drags
retain their prior behavior. The receiver starts a native copy-only drag with the
completed file URL. Quitting either control-panel app disables the drag handoff; ordinary
mouse and keyboard sharing can continue.

The edge indicator is a compact macOS material card with native circular progress,
a filename, and a checkmark plus directional arrow when ready. Its short fades and
ready transition do not delay crossing; Reduce Motion disables animation, and
Reduce Transparency uses a solid background. It never takes focus or intercepts
mouse input.

The bridge starts with the menu-bar app, even if the main window stays closed.
While file sharing is enabled and the local daemon is connected, its input listener
prevents App Nap from delaying the handoff timer; normal system sleep remains
allowed. The receiving drag starts through a temporary native source window's
mouse-event handler. Per-transfer `handoff` diagnostics distinguish `starting`,
`dragging`, `copied`, and `cancelled`; the daemon's `activated` status alone only
confirms delivery of the request, not acceptance by the destination app. A `copied`
result means the app accepted the native drop, not that a chat message was sent or
a server-side upload finished.
The receiver adopts the already-held button without clicking the app underneath
the crossing point. This requires file-bridge v2 on both Macs and prevents a
window border from grabbing the gesture as a resize before the file drag starts.
The daemon verifies that the relay's temporary window covers the click point,
then posts a real mouse-down through the normal HID path so macOS tracks the held
button. A process-addressed click alone does not hold the button for native dragging.
Startup retries cannot double-click or revive a released drag. The relay cancels abandoned sessions
after release, return to the other Mac, disconnection, or timeout, rather than
leaving a file icon on screen.

Only the daemon reads the source file, as it approaches the shared edge. The UI
does not inspect its contents or attributes, avoiding separate folder-access
requests from two processes. macOS may still request access the first time the
daemon reads from a protected folder; respect that system prompt. Source drag
cancellation also runs in the daemon, using its existing Accessibility permission.

For builds that retain macOS privacy grants across updates, set
`LAN_MOUSE_APP_SIGN_IDENTITY` to a persistent code-signing identity on the first
build. `build.sh` preserves the installed app's signing identity on subsequent
builds. Ad-hoc signing remains a development fallback, but its identity changes
with the executable and cannot preserve the same permission identity.

## Panels

The compact control panel uses a native macOS sidebar. Page changes preserve
unsaved control edits and pairing progress. Sharing has one consistent toggle;
local service availability is never labeled as a remote connection.

- **Home:** a plain-language connection summary and the next useful action.
  Connection Health expands to network RTT and its chart; route, address,
  average/p95 and loss sit inside Network Details. ICMP probes measure the
  network, not end-to-end input latency, and can be blocked by a firewall.
- **Computers:** paired computer cards, connection status, resume, and per-Mac
  actions. Add Mac starts a four-step Prepare → Choose Mac → Verify → Try It
  flow. Incoming pairing invitations appear above every page with Review and Dismiss. Dismiss hides that request across refreshes and app restarts without changing sharing or trust; a new invitation from the same Mac can still appear. The verification screen also offers Dismiss before approval. Advanced
  connectivity settings remain collapsed. Incoming access has its own
  disclosure and identity details; destructive actions require a live service.
- **Arrangement:** drag computers to the edge matching your physical desk or
  choose an edge from a menu. Replacing an occupied edge requires confirmation.
  Positions are local; arrange the inverse direction on the other Mac.
- **Mouse:** the bundled MMF engine provides scrolling and extra-button actions
  inside Lan Mouse, with MMF-style **Buttons** and **Scrolling** tabs. A grouped
  action table includes click, double-click, hold, drag and scroll gestures,
  effect menus, shortcut recording, Add Action, Options and Restore Defaults.
  **Scroll & Navigate** is the two-finger pan gesture; **Spaces & Mission Control**
  and **Zoom In or Out** are the other drag choices. Drag zoom uses upward
  movement to enlarge and downward movement to shrink. Scrolling includes keyboard-modifier recording,
  precision and trackpad simulation. Preferences follow the mouse across paired
  Macs; custom rows are preserved. No standalone MMF app or second menu icon is needed. Permission and
  engine recovery controls appear only when needed. See [engine setup](MMF-INTEGRATION.md).
- **Keys & Buttons:** choose a named Mac, then add an explicit key-to-key or
  button-to-button rule. New rules start with empty choices; the sheet adds to a
  draft and Save Changes applies it. A persistent footer offers save, retry and
  discard. Drafts belong to their selected Mac and survive page navigation;
  switching computers with edits asks before discarding. Rules can be drafted
  while sharing is off, but saving requires the local sharing service. The reviewed editor stays separate from the shared Mouse panel.
- **Troubleshooting:** service and input-readiness checks, restart, setup help,
  a focused input test, connection events, service logs, and report export.
  The input pad only listens while its page is visible and it has focus.
  Recorded input stays in memory and is excluded from exported reports.

The menu-bar mouse icon opens a 236-point popover with a small native **Sharing**
switch, a green/gray remote-connection light, and short panel shortcuts. It uses
13-point system text, compact rows, and system selection colors. Green requires
a live remote peer, not merely a running local service. Hover highlights the
whole row; choosing a panel closes the popover. The switch stays open and gives
immediate Starting/Stopping feedback. Live latency stays in Home. The footer
explains that quitting the control panel leaves sharing running.

Pausing sharing retains computers already loaded in this app session; their
live status resets and editing is disabled until reconnection. After launching
the app with sharing off, turn sharing on to load saved computers from the service.

Unchanged daemon status no longer redraws every panel; socket writes and log
reads run off the UI thread. Mapping saves finish when the update arrives, with
a five-second upper bound instead of a fixed one-second delay.
Quitting the panel leaves sharing unchanged. Turning sharing off disables and
unloads the user's `de.feschber.lan-mouse` launch agent; it stays disabled across
login until turned back on. Turning it on enables and loads that launch agent.
The app expects the default `~/.config/lan-mouse/config.toml`,
`~/Library/Caches/lan-mouse-socket.sock`, and launch-agent paths.

## Pair two Macs

1. Open Lan Mouse Control on both Macs, then start **Connect a Mac** (or **Computers → Add Mac…**) on either one. **Prepare** explains the two macOS permissions,
   links to each Settings page, and checks that capture and emulation are enabled.
2. Select the other Mac by name and place it beside This Mac in the desk diagram. Click
   **Continue**. On the other Mac, click **Review** in the invitation banner; its
   opposite position is filled in automatically. Live progress shows when the other Mac opens the code or approves; Send invitation again retries delivery without changing the code.
3. Compare all six groups of the verification code on both screens. Click
   **The codes match — connect** on each Mac. No access is granted by discovery
   or by receiving an invitation alone.
4. Follow **Try sharing** to move across the chosen edge and back. Confirm
   **It works!** after testing the mouse; a responding peer alone is not proof
   that input was delivered correctly.

**Advanced connection settings** is collapsed by default. It retains each Mac’s
hostname, port, full fingerprint and copy button. In **Choose a Mac**, expand it
for manual hostname/IP, port, position, fingerprint verification and connection.
Manual fallback still requires configuration on both Macs.

Discovery uses Bonjour TXT updates, including a five-minute invitation with a
random UUID, target fingerprint, and position. The comparison code is the first
96 bits of SHA-256 over both sorted, normalized full fingerprints and the
invitation UUID. Names, addresses and advertised requests are untrusted; users
must compare the entire code on the two physical screens before granting access.
Changing an identity or invitation invalidates confirmation. Discovery progress is informational only and never grants access; approval still requires the full code comparison on both Macs. TXT monitoring starts after resolution and consumes callback data, including cancellation updates. Manual connections
continue to require comparison of the full fingerprint. The daemon’s existing
certificate checks enforce the approved identity during input sharing.

Pairing stores a hostname when available, so DHCP address changes can resolve
through mDNS. Existing manually configured fallback addresses are preserved.
If a firewall is enabled, it must allow the daemon's UDP port (normally 4242).

## Disconnect or offboard

**Pause Sending to This Mac** disables outgoing sharing but keeps position and
mappings; the global Sharing switch pauses all input sharing on this Mac.
**Remove Mac…** removes the saved computer. For connections paired in this
version, it also revokes that computer’s approved incoming identity automatically.
The identity association comes only from confirmed pairing, never from discovery.
Older connections have a friendly-name access selector because that association
was not saved; identity details remain folded away. Revocation briefly restarts
sharing to close existing sessions. Repeat Remove on the other Mac to clear its
saved connection too. Removing the last computer persists across restarts.
Nothing in these flows deletes the device’s own identity certificate.

After the mouse test, Done returns Home and Computers opens the management list.
Completion explains shared mouse preferences; both Macs must enable the Mouse
panel’s shared-settings switch for synchronization. Existing preferences are
preserved by pairing.

## Manual checks

Verify the on/off switch restores the service, drag and menu positioning agree,
a mapping survives a daemon restart, malformed mappings are rejected, and a
focused Option+Up/mouse/scroll test reports events. Pair two instances after
comparing fingerprints, verify an unapproved peer cannot control the machine,
and revoke a test identity while connected. Check discovery removal when the
other app closes, and ensure offline/timeout states never display a healthy RTT.

All disclosure headers are clickable across their full width and retain keyboard
interaction. Computer action menus show only the ellipsis, without a down caret.

Native file-drag regression check (macOS, interactive desktop): `NativeDragSmoke.swift`
creates two disposable test windows. `examples/native_file_drag_check.rs` uses the
production capture/emulation APIs to hold a drag, move it into the test destination,
and release or cancel it. The destination verifies the exact fixture URL and contents.
The driver also rejects duplicate presses and released-button startup. This test
caught both windowless process events and premature native drops before release.

Build the Rust driver with `cargo build --no-default-features --example native_file_drag_check`.
Build the test app with:

```sh
xcrun swiftc -swift-version 5 -parse-as-library macos-ui/ControlModel.swift macos-ui/Discovery.swift macos-ui/MouseProfile.swift macos-ui/SharingShortcut.swift macos-ui/FileBridge.swift macos-ui/NativeDragSmoke.swift -o /tmp/NativeDragSmoke
```

Run `python3 macos-ui/test-native-drag.py --driver <target-dir>/debug/examples/native_file_drag_check --app /tmp/NativeDragSmoke --identity <existing-daemon-signing-identity> --keychain <signing-keychain>`;
add `--cancel` to verify cancellation. The runner temporarily pauses the local sharing
service and restores its previous state, closes both test windows, and prints its
artifact directory. Use the daemon's existing signing identity so its Accessibility
grant can be used where macOS permits it. Some Macs require a separate grant for a
standalone test executable; if macOS rejects it, use the installed app for physical
verification instead of changing privacy settings automatically. This is a native receiver test; a physical cross-Mac Orca/Slack
drop is still a separate end-to-end check.
