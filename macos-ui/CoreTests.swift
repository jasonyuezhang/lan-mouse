import Foundation
import AppKit
import Combine

final class ResolvedTestService: NetService {
    override var hostName: String? { "test.local." }
}

@main struct CoreTests {
    @MainActor static func main() async {
        _ = NSApplication.shared
        let dragScreen = NSRect(x: 0, y: 0, width: 1000, height: 800)
        for (side, near, away) in [
            ("left", NSPoint(x: 150, y: 400), NSPoint(x: 200, y: 400)),
            ("right", NSPoint(x: 850, y: 400), NSPoint(x: 800, y: 400)),
            ("top", NSPoint(x: 500, y: 650), NSPoint(x: 500, y: 600)),
            ("bottom", NSPoint(x: 500, y: 150), NSPoint(x: 500, y: 200))
        ] {
            assert(shouldPrepareFileDrag(position: side, mouse: near, previous: away, screen: dragScreen))
            assert(!shouldPrepareFileDrag(position: side, mouse: away, previous: near, screen: dragScreen))
            assert(!shouldPrepareFileDrag(position: side, mouse: near, previous: nil, screen: dragScreen))
            assert(!shouldPrepareFileDrag(position: side, mouse: NSPoint(x: 500, y: 400), previous: away, screen: dragScreen))
        }
        assert(shouldPrepareFileDrag(position: "left", mouse: NSPoint(x: 10, y: 400), previous: nil, screen: dragScreen))
        assert(chromeProfileName(windowTitle: "Nash - Calendar - Week of September 20, 2026 - Google Chrome - Jason (Nash)") == "Nash")
        assert(chromeProfileName(windowTitle: "Synced Flow (Piano) - YouTube - Audio playing - Google Chrome - Yue (Jason)") == "Jason")
        assert(chromeProfileName(windowTitle: "Inbox - Google Chrome - Jason") == "Jason")
        assert(chromeProfileName(windowTitle: "Who’s using Chrome?") == nil)
        let localState = Data(#"{"profile":{"info_cache":{"Default":{"name":"Jason"},"Profile 3":{"name":"Nash"}}}}"#.utf8)
        assert(chromeProfileDirectory(named: "Nash", localState: localState) == "Profile 3")
        assert(chromeProfileDirectory(named: "Jason", localState: localState) == "Default")
        assert(chromeProfileDirectory(named: "Work", localState: localState) == nil)
        assert(ChromeHandoff(url: "https://github.com/feschber/lan-mouse", profile: "Nash").webURL != nil)
        for url in ["file:///etc/passwd", "javascript:alert(1)", "chrome://settings", "https://", "not a url"] {
            assert(ChromeHandoff(url: url, profile: nil).webURL == nil)
        }
        let shortcutSuite = "lan-mouse-shortcut-tests-" + UUID().uuidString
        let shortcutPrefs = UserDefaults(suiteName: shortcutSuite)!
        defer { shortcutPrefs.removePersistentDomain(forName: shortcutSuite) }
        let shortcut = SharingShortcut(keyCode: 111, modifiers: (1 << 18) | (1 << 19) | (1 << 20)) // ⌃⌥⌘F12
        assert(shortcut.valid)
        for invalid in [SharingShortcut(keyCode: 40, modifiers: 0), SharingShortcut(keyCode: 40, modifiers: 1 << 17), SharingShortcut(keyCode: 40, modifiers: SharingShortcut.allowedModifiers), SharingShortcut(keyCode: 59, modifiers: 1 << 18)] { assert(!invalid.valid) }
        let shortcuts = SharingShortcutController(preferences: shortcutPrefs)
        shortcuts.start()
        assert(shortcuts.save(shortcut), shortcuts.error ?? "Registration failed")
        assert(shortcuts.active == shortcut)
        assert(SharingShortcutController(preferences: shortcutPrefs).shortcut == shortcut)
        let competingPrefs = UserDefaults(suiteName: shortcutSuite + "-conflict")!
        defer { competingPrefs.removePersistentDomain(forName: shortcutSuite + "-conflict") }
        let competing = SharingShortcutController(preferences: competingPrefs)
        competing.start()
        assert(!competing.save(shortcut))
        assert(competing.shortcut == nil)
        var presses = 0
        shortcuts.onPress = { presses += 1 }
        let wire = try! JSONSerialization.jsonObject(with: JSONSerialization.data(withJSONObject: shortcut.ipc)) as! [String: Any]
        shortcuts.captured(["key_code": 0, "modifiers": 0]); assert(presses == 0)
        shortcuts.captured(wire); shortcuts.captured(wire); assert(presses == 1)
        shortcuts.suspend(); assert(shortcuts.active == nil)
        shortcuts.resume(); assert(shortcuts.active == shortcut)
        assert(shortcuts.save(nil)); assert(shortcuts.active == nil)
        assert(shortcutPrefs.data(forKey: "sharingShortcut") == nil)
        let mouseFixture: [String: Any] = [
            "Constants": ["configVersion": 24], "License": ["localSecret": "test"], "State": ["tab": "local"],
            "General": ["scrollKillSwitch": false, "buttonKillSwitch": false, "lockPointerDuringDrag": true, "showMenuBarItem": true],
            "Scroll": ["smooth": "off"], "Pointer": ["sensitivity": 1], "Remaps": [[String: Any]]()
        ]
        let shared = try! sharedMouseSettings(mouseFixture)
        assert(Set(shared.keys) == Set(["General", "Scroll", "Pointer", "Remaps"]))
        assert((shared["General"] as! [String: Any])["showMenuBarItem"] == nil)
        assert((shared["General"] as! [String: Any])["lockPointerDuringDrag"] as? Bool == true)
        var incompatible = mouseFixture
        incompatible["Constants"] = ["configVersion": 25]
        assert((try? sharedMouseSettings(incompatible)) == nil)
        assert((try? mouseConfiguration(Data("[]".utf8))) == nil)
        let mouseModel = MouseProfileModel()
        mouseModel.config = ["Remaps": [
            ["trigger": ["button": 3, "level": 1, "duration": "click"], "modifiers": [:], "effect": ["type": "smartZoom"]],
            ["trigger": ["button": 3, "level": 2, "duration": "click"], "modifiers": [:], "effect": ["type": "symbolicHotkey", "variant": 32]],
            ["trigger": ["button": 3, "level": 1, "duration": "hold"], "modifiers": [:], "effect": ["type": "symbolicHotkey", "variant": 36]],
            ["trigger": "dragTrigger", "modifiers": ["buttonModifiers": [["button": 3, "level": 1]]], "effect": ["modifiedDragType": "twoFingerSwipe"]]
        ]]
        assert(mouseModel.buttonAction(3) == "smartZoom")
        assert(mouseModel.buttonAction(3, clicks: 2) == "hotkey:32")
        assert(mouseModel.buttonAction(3, duration: "hold") == "hotkey:36")
        assert(mouseModel.buttonAction(4) == "default")
        let pan = MouseRemap.make(button: 5, gesture: "dragTrigger", effect: ["modifiedDragType": "twoFingerSwipe"])
        assert(pan.button == 5 && pan.title == "Click and Drag" && pan.actionTitle == "Scroll & Navigate")
        assert(pan.actions.map(\.title) == ["Scroll & Navigate", "Spaces & Mission Control", "Zoom In or Out"])
        let dragZoom = MouseRemap.make(button: 4, gesture: "dragTrigger", effect: ["modifiedDragType": "zoom"])
        assert(dragZoom.button == 4 && dragZoom.title == "Click and Drag" && dragZoom.actionID == "zoom")
        let custom = MouseRemap.make(button: 4, gesture: "scrollTrigger", flags: 524288, effect: ["futureEffect": "keep me"])
        assert(custom.title == "⌥Click and Scroll" && custom.actionID == "custom")
        var edited = mouseFixture
        edited["Remaps"] = [custom.value, pan.value]
        let spaces = MouseRemap.make(button: 5, gesture: "dragTrigger", effect: ["modifiedDragType": "threeFingerSwipe"])
        try! replaceMouseRemap(in: &edited, old: pan, new: spaces)
        assert(NSDictionary(dictionary: (edited["Remaps"] as! [[String: Any]])[0]).isEqual(to: custom.value))
        assert((edited["License"] as! [String: String])["localSecret"] == "test")
        do { try replaceMouseRemap(in: &edited, old: pan, new: nil); assertionFailure("A stale edit must fail") } catch { }
        do { try replaceMouseRemap(in: &edited, old: nil, new: spaces); assertionFailure("Duplicate gestures must fail") } catch { }
        try! replaceMouseRemap(in: &edited, old: spaces, new: nil)
        assert((edited["Remaps"] as! [[String: Any]]).count == 1)
        let double = MouseRemap.make(button: 3, gesture: "double", effect: ["type": "smartZoom"])
        assert(double.title == "Double Click" && double.trigger["level"] as? Int == 2)
        edited["Scroll"] = ["smooth": "high", "modifiers": defaultMouseScrollModifiers]
        setMouseScrollModifier(in: &edited, key: "zoom", flags: 524288)
        let updatedScroll = edited["Scroll"] as! [String: Any]
        let updatedModifiers = updatedScroll["modifiers"] as! [String: Any]
        assert(updatedScroll["smooth"] as? String == "high")
        assert(updatedModifiers["zoom"] as? UInt == 524288 && updatedModifiers["precise"] as? UInt == 0)
        assert(updatedModifiers["horizontal"] as? UInt == 131072)
        let lockDirectory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let lockState = lockDirectory.appendingPathComponent("mouse-profile.json")
        try! withMouseProfileLock(lockState) { assert(FileManager.default.fileExists(atPath: lockDirectory.appendingPathComponent("mouse-profile.lock").path)) }
        try! FileManager.default.removeItem(at: lockDirectory)
        for _ in 0..<10 {
            let commandResult = await command("/usr/bin/true", [])
            assert(commandResult.code == 0)
        }
        let start = Date()
        let timeoutResult = await command("/bin/sleep", ["5"], timeout: 0.1)
        assert(timeoutResult.code != 0 && Date().timeIntervalSince(start) < 2)
        assert(pingTime("64 bytes: time=12.345 ms") == 12.345)
        assert(pingTime("time<1 ms") == 1)
        assert(pingTime("Request timeout") == nil)
        let fp = String(repeating: "ab", count: 32)
        let normalized = normalizedFingerprint(fp)!
        assert(normalized.split(separator: ":").count == 32)
        assert(normalizedFingerprint(normalized.uppercased()) == normalized)
        assert(normalizedFingerprint("abc") == nil)
        assert(normalizedFingerprint(String(repeating: "zz", count: 32)) == nil)
        let other = String(repeating: "cd", count: 32)
        let invitation = "01234567-89AB-CDEF-0123-456789ABCDEF"
        let code = pairingCode(fp, other, invitation: invitation)!
        assert(code == pairingCode(other, fp, invitation: invitation))
        assert(code.split(separator: " ").count == 6)
        assert(code != pairingCode(fp, other, invitation: UUID().uuidString))
        assert(pairingCode(fp, fp, invitation: invitation) == nil)
        assert(pairingCode(fp, other, invitation: "invalid") == nil)
        assert(oppositePosition("left") == "right" && oppositePosition("top") == "bottom")
        assert(oppositePosition("unknown") == nil)
        let discovery = Discovery()
        let id = discovery.invite(normalized, position: "left")
        assert(discovery.invitationIsActive(id))
        discovery.cancelInvitation()
        assert(!discovery.invitationIsActive(id))
        let service = ResolvedTestService(domain: "local.", type: "_lanmouse._udp.", name: "Test Mac", port: 4242)
        let initialRecord = NetService.data(fromTXTRecord: ["fingerprint": Data(normalized.utf8)])
        discovery.netService(service, didUpdateTXTRecord: initialRecord)
        assert(discovery.nearby.count == 1 && discovery.nearby[0].invitationID == nil)
        let updatedRecord = NetService.data(fromTXTRecord: ["fingerprint": Data(normalized.utf8), "invitation": Data(invitation.utf8),
            "target": Data(other.utf8), "position": Data("left".utf8), "progressTarget": Data(other.utf8),
            "progressID": Data(invitation.utf8), "progressStage": Data("approved".utf8)])
        discovery.netService(service, didUpdateTXTRecord: updatedRecord)
        assert(discovery.nearby[0].invitationID == invitation, "Use updated callback data even when the service cache is stale")
        assert(discovery.nearby[0].progress(for: invitation, target: other) == "approved")
        assert(discovery.nearby[0].progress(for: UUID().uuidString, target: other) == nil)
        assert(discovery.nearby[0].progress(for: invitation, target: fp) == nil)
        discovery.netService(service, didUpdateTXTRecord: initialRecord)
        assert(discovery.nearby[0].invitationID == nil && discovery.nearby[0].progressStage == nil, "Cancelled invitations must disappear")
        let row: [Any] = [7, ["hostname": "test.local", "fix_ips": ["192.0.2.1"], "pos": "right", "key_map": ["58": 1]],
            ["active": true, "alive": true, "active_addr": "[2001:db8::1]:4242", "peer_commit": Array("12345678".utf8)]]
        let peer = Peer(row)!
        assert(peer.id == 7 && peer.position == "right" && peer.target == "2001:db8::1")
        assert(peer.mappings == ["58": 1] && peer.version == "12345678")
        var draft = MappingDraft()
        draft.load(peer)
        assert(!draft.dirty && draft.peerID == peer.id)
        draft.rows[0].to = 2
        var acknowledgedPeer = peer
        acknowledgedPeer.mappings = ["58": 2]
        assert(draft.matchesSaved(acknowledgedPeer), "Late matching acknowledgement must confirm a save")
        assert(!draft.matchesSaved(peer), "Different saved rules remain a conflict")
        assert(draft.dirty && draft.dictionary == ["58": 2])
        draft.rows[0].to = 1
        assert(!draft.dirty, "Undoing a change must clear unsaved state")
        draft.rows.append(MappingRow(from: 58, to: 1))
        assert(draft.duplicate && draft.dirty, "Duplicate sources cannot be saved")
        draft.load(peer)
        draft.rows.removeAll()
        assert(draft.dirty && draft.dictionary.isEmpty, "Removing the last rule is a savable change")
        draft.load(peer)
        assert(Peer([1]) == nil)
        let suite = "lan-mouse-tests-" + UUID().uuidString
        let preferences = UserDefaults(suiteName: suite)!
        defer { preferences.removePersistentDomain(forName: suite) }
        let model = ControlModel(preferences: preferences)
        model.consume(try! JSONSerialization.data(withJSONObject: ["Enumerate": [row]]))
        assert(model.peers.count == 1 && model.selected == 7)
        model.selected = 99
        assert(draft.peerID == 7 && draft.loaded == peer.mappings, "Other pages must not retarget a draft")
        model.selected = 7
        assert(!model.hasRemoteConnection)
        model.connected = true
        assert(model.hasRemoteConnection)
        model.peers[0].alive = false
        assert(!model.hasRemoteConnection)
        model.peers[0].alive = true
        model.consume(Data(#"{"CaptureStatus":"Enabled"}"#.utf8))
        assert(model.capture == "Enabled")
        model.consume(Data(#"{"AuthorizedUpdated":{"ab":"test"}}"#.utf8))
        assert(model.authorized == ["ab": "test"])
        var redraws = 0
        let observation = model.objectWillChange.sink { redraws += 1 }
        model.consume(try! JSONSerialization.data(withJSONObject: ["Enumerate": [row]]))
        model.consume(Data(#"{"CaptureStatus":"Enabled"}"#.utf8))
        model.consume(Data(#"{"AuthorizedUpdated":{"ab":"test"}}"#.utf8))
        assert(redraws == 0, "Identical status must not redraw every screen")
        model.requestedSharing = true
        assert(model.sharingEnabled)
        model.requestedSharing = false
        assert(!model.sharingEnabled)
        observation.cancel()
        assert(peer.matches(host: "TEST.local.", port: 4242))
        assert(!peer.matches(host: "test.local", port: 4243))
        assert(model.verifiedIdentity(for: peer) == nil, "Discovery alone must not associate incoming access")
        model.authorized = [normalized: "Test Mac"]
        model.rememberIdentity(fp.uppercased(), for: peer)
        assert(model.verifiedIdentity(for: peer) == normalized)
        let reopened = ControlModel(preferences: preferences)
        reopened.authorized = model.authorized
        assert(reopened.verifiedIdentity(for: peer) == normalized, "Confirmed identity survives reopening")
        model.consume(Data(#"{"Deleted":7}"#.utf8))
        assert(model.verifiedIdentity(for: peer) == nil, "Deleted endpoints must not retain an identity link")
        assert(model.peers.isEmpty)
        model.requestedSharing = nil
        model.running = false
        assert(model.homeTitle == "Your desk, on pause")
        model.running = true; model.connected = false
        assert(model.homeTitle == "Connecting to sharing…")
        model.connected = true
        assert(model.homeTitle == "One mouse. Both Macs.")
        model.consume(try! JSONSerialization.data(withJSONObject: ["Enumerate": [row]]))
        assert(model.homeTitle == "Your Macs are connected")
        model.peers[0].alive = false
        assert(model.homeTitle == "Ready when your other Mac is")
        assert(model.displayName(for: peer) == "test.local")
        model.discovery.nearby = [NearbyMac(id: "test", name: "My Other Mac", host: "test.local.", ips: ["192.0.2.1"], port: 4242, fingerprint: normalized)]
        assert(model.displayName(for: peer) == "My Other Mac")
        model.fingerprint = normalized
        var request = NearbyMac(id: "request", name: "Other Mac", host: "other.local.", ips: [], port: 4242,
                                fingerprint: normalizedFingerprint(other)!, invitationTarget: normalized, invitationPosition: "left", invitationID: invitation)
        assert(model.canReviewInvitation(request))
        let existingPeers = model.peers, existingTrust = model.authorized, currentPanel = model.panel
        model.invitationToReview = invitation
        model.dismissInvitation(request)
        assert(!model.canReviewInvitation(request) && model.invitationToReview == nil)
        assert(model.peers == existingPeers && model.authorized == existingTrust && model.panel == currentPanel,
               "Dismissing a request must not alter sharing, trust, or the current page")
        let afterDismiss = ControlModel(preferences: preferences)
        afterDismiss.fingerprint = normalized
        assert(!afterDismiss.canReviewInvitation(request), "Dismissed requests must stay hidden after reopening or discovery refresh")
        request.invitationID = UUID().uuidString
        assert(afterDismiss.canReviewInvitation(request), "A new request from the same Mac is still allowed")
        request.invitationTarget = other
        afterDismiss.dismissInvitation(request)
        assert(afterDismiss.dismissedInvitations == model.dismissedInvitations, "Ignore requests meant for another Mac")
        print("PASS: mouse profile scope/compatibility/locking, IPC state, mapping decode, IPv6 endpoints, fingerprints, pairing codes/invitations, and latency parsing")
    }
}
