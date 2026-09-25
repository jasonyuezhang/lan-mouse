import AppKit
import SwiftUI
import CoreFoundation
import Darwin

// MMF 3.1's profile format. Licenses, local permissions, identities, and app UI
// preferences are deliberately absent from this allowlist.
func sharedMouseSettings(_ config: [String: Any]) throws -> [String: Any] {
    guard (config["Constants"] as? [String: Any])?["configVersion"] as? Int == 24,
          let scroll = config["Scroll"] as? [String: Any],
          let pointer = config["Pointer"] as? [String: Any],
          let remaps = config["Remaps"] as? [[String: Any]],
          let general = config["General"] as? [String: Any] else {
        throw NSError(domain: "MouseProfile", code: 1, userInfo: [NSLocalizedDescriptionKey: "Mac Mouse Fix’s configuration is incomplete."])
    }
    let sharedGeneral = general.filter { ["scrollKillSwitch", "buttonKillSwitch", "lockPointerDuringDrag"].contains($0.key) }
    guard sharedGeneral.count == 3, sharedGeneral.values.allSatisfy({ $0 is Bool }) else {
        throw CocoaError(.propertyListReadCorrupt)
    }
    return ["Scroll": scroll, "Pointer": pointer, "Remaps": remaps,
            "General": sharedGeneral]
}

// Coordinate with the daemon; MMF itself uses its file watcher and does not lock.
func withMouseProfileLock<T>(_ stateURL: URL, _ body: () throws -> T) throws -> T {
    try FileManager.default.createDirectory(at: stateURL.deletingLastPathComponent(), withIntermediateDirectories: true)
    let lockURL = stateURL.deletingPathExtension().appendingPathExtension("lock")
    let fd = open(lockURL.path, O_CREAT | O_RDWR, 0o600)
    guard fd >= 0 else { throw CocoaError(.fileWriteNoPermission) }
    defer { close(fd) }
    guard flock(fd, LOCK_EX) == 0 else { throw CocoaError(.fileWriteUnknown) }
    defer { flock(fd, LOCK_UN) }
    return try body()
}

func mouseConfiguration(_ data: Data) throws -> [String: Any] {
    guard let values = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else {
        throw CocoaError(.propertyListReadCorrupt)
    }
    _ = try sharedMouseSettings(values)
    return values
}

@MainActor final class MouseProfileModel: ObservableObject {
    @Published var config: [String: Any] = [:]
    @Published var syncEnabled = false
    @Published var revision = 0
    @Published var error: String?
    @Published var busy = false
    @Published var installed = false
    @Published var helperRunning = false
    @Published var accessibilityGranted: Bool?
    static var engineURL: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Library/MouseEngine.bundle") }
    static var helperURL: URL { engineURL.appendingPathComponent("Contents/Library/LoginItems/Mac Mouse Fix Helper.app") }
    var engineService: String { "gui/\(getuid())/de.feschber.lan-mouse.mouse-engine" }
    func restartEngine() {
        guard !busy else { return }; busy = true; error = nil
        Task { await ensureEngine(restart: true); busy = false; await refresh() }
    }
    private func ensureEngine(restart: Bool = false) async {
        guard FileManager.default.isExecutableFile(atPath: Self.helperURL.appendingPathComponent("Contents/MacOS/Mac Mouse Fix Helper").path) else { return }
        let executable = Self.helperURL.appendingPathComponent("Contents/MacOS/Mac Mouse Fix Helper").path
        let plist = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/de.feschber.lan-mouse.mouse-engine.plist")
        do {
            let data = try PropertyListSerialization.data(fromPropertyList: [
                "Label": "de.feschber.lan-mouse.mouse-engine", "ProgramArguments": [executable],
                "RunAtLoad": true, "KeepAlive": true, "ProcessType": "Interactive",
                "StandardOutPath": "/tmp/lan-mouse-mouse-engine.log", "StandardErrorPath": "/tmp/lan-mouse-mouse-engine.log"
            ] as [String: Any], format: .xml, options: 0)
            let changed = (try? Data(contentsOf: plist)) != data
            if changed {
                try FileManager.default.createDirectory(at: plist.deletingLastPathComponent(), withIntermediateDirectories: true)
                try data.write(to: plist, options: .atomic)
                _ = await command("/bin/launchctl", ["bootout", engineService])
            }
            let status = await command("/bin/launchctl", ["print", engineService])
            let result: CommandResult
            if status.code != 0 { result = await command("/bin/launchctl", ["bootstrap", "gui/\(getuid())", plist.path]) }
            else if restart { result = await command("/bin/launchctl", ["kickstart", "-k", engineService]) }
            else { return }
            if result.code != 0 { error = "The mouse engine couldn’t start. Use Retry to try again." }
        } catch { self.error = "Couldn’t prepare the mouse engine: \(error.localizedDescription)" }
    }
    private var loadedData: Data?
    private var started = false
    let configURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/com.nuebling.mac-mouse-fix/config.plist")
    let stateURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config/lan-mouse/mouse-profile.json")
    var scroll: [String: Any] { config["Scroll"] as? [String: Any] ?? [:] }
    var pointer: [String: Any] { config["Pointer"] as? [String: Any] ?? [:] }
    var general: [String: Any] { config["General"] as? [String: Any] ?? [:] }
    var compatible: Bool { (config["Constants"] as? [String: Any])?["configVersion"] as? Int == 24 }

    func start() {
        guard !started else { return }; started = true
        Task {
            await ensureEngine()
            while !Task.isCancelled {
                await refresh()
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
    }
    func refresh() async {
        let configURL = configURL, stateURL = stateURL, helperURL = Self.helperURL
        let snapshot = await Task.detached(priority: .utility) {
            (try? Data(contentsOf: configURL), try? Data(contentsOf: stateURL), FileManager.default.fileExists(atPath: helperURL.path), mouseEngineAccessibility())
        }.value
        if installed != snapshot.2 { installed = snapshot.2 }
        let running = NSRunningApplication.runningApplications(withBundleIdentifier: "com.nuebling.mac-mouse-fix.helper").contains { !$0.isTerminated && $0.bundleURL?.standardizedFileURL == Self.helperURL.standardizedFileURL }
        if helperRunning != running { helperRunning = running }
        if accessibilityGranted != snapshot.3 { accessibilityGranted = snapshot.3 }
        if let data = snapshot.0, loadedData != data {
            if let values = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] {
                loadedData = data; config = values
            }
        }
        let state = snapshot.1.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }
        let enabled = state?["enabled"] as? Bool ?? false
        if syncEnabled != enabled { syncEnabled = enabled }
        let currentRevision = (state?["document"] as? [String: Any])?["revision"] as? Int ?? 0
        if revision != currentRevision { revision = currentRevision }
    }
    func set(_ section: String, _ key: String, _ value: Any) {
        edit { config in
            var values = config[section] as? [String: Any] ?? [:]
            values[key] = value; config[section] = values
        }
    }
    func edit(_ change: @escaping (inout [String: Any]) throws -> Void) {
        guard !busy, compatible else { return }; busy = true; error = nil
        let url = configURL, stateURL = stateURL
        Task {
            do {
                try await Task.detached(priority: .utility) {
                    try withMouseProfileLock(stateURL) {
                    let original = try Data(contentsOf: url)
                    var values = try mouseConfiguration(original)
                    try change(&values)
                    _ = try sharedMouseSettings(values)
                    let backup = url.deletingLastPathComponent().appendingPathComponent("config.before-lan-mouse-edit.plist")
                    let backupFD = open(backup.path, O_CREAT | O_EXCL | O_WRONLY, 0o600)
                    if backupFD >= 0 {
                        let file = FileHandle(fileDescriptor: backupFD, closeOnDealloc: true)
                        try file.write(contentsOf: original)
                        try file.close()
                    } else if errno != EEXIST { throw CocoaError(.fileWriteNoPermission) }
                    // Avoid overwriting an edit made by the daemon or MMF while this form was saving.
                    guard try Data(contentsOf: url) == original else { throw CocoaError(.fileWriteUnknown) }
                    let data = try PropertyListSerialization.data(fromPropertyList: values, format: .xml, options: 0)
                    try data.write(to: url, options: .atomic)
                    }
                    notifyMouseFix()
                }.value
                await refresh()
            } catch { self.error = "Mouse settings were not saved: \(error.localizedDescription)" }
            busy = false
        }
    }
    func setSync(_ enabled: Bool) { setSync(enabled, initialRevision: 1) }
    func setSync(_ enabled: Bool, initialRevision: Int) {
        guard !busy, compatible, !enabled || installed else { return }; busy = true; error = nil
        let configURL = configURL, stateURL = stateURL
        Task {
            do {
                try await Task.detached(priority: .utility) {
                    try withMouseProfileLock(stateURL) {
                    let data = try Data(contentsOf: configURL)
                    let values = try mouseConfiguration(data)
                    var state: [String: Any] = [:]
                    if FileManager.default.fileExists(atPath: stateURL.path) {
                        guard let saved = try JSONSerialization.jsonObject(with: Data(contentsOf: stateURL)) as? [String: Any],
                              saved["node"] is String, saved["document"] is [String: Any] else { throw CocoaError(.propertyListReadCorrupt) }
                        state = saved
                    }
                    if state.isEmpty {
                        let node = UUID().uuidString
                        state = ["node": node, "document": ["schema": 1, "revision": initialRevision, "author": node, "settings": try sharedMouseSettings(values)]]
                    }
                    state["enabled"] = enabled
                    try FileManager.default.createDirectory(at: stateURL.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try JSONSerialization.data(withJSONObject: state, options: [.prettyPrinted, .sortedKeys]).write(to: stateURL, options: .atomic)
                    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: stateURL.path)
                    }
                }.value
                await refresh()
            } catch { self.error = "Could not change settings sync: \(error.localizedDescription)" }
            busy = false
        }
    }
    func buttonAction(_ button: Int, clicks: Int = 1, duration: String = "click") -> String {
        guard let row = (config["Remaps"] as? [[String: Any]])?.first(where: { simpleTrigger($0, button, clicks: clicks, duration: duration) }),
              let effect = row["effect"] as? [String: Any] else { return "default" }
        if effect["type"] as? String == "symbolicHotkey", let variant = effect["variant"] as? Int, [32,33,36].contains(variant) { return "hotkey:\(variant)" }
        if effect["type"] as? String == "smartZoom" { return "smartZoom" }
        return "custom"
    }
    func setButtonAction(_ button: Int, _ action: String, clicks: Int = 1, duration: String = "click") {
        guard ["default", "smartZoom", "hotkey:32", "hotkey:33", "hotkey:36"].contains(action) else { return }
        edit { config in
            var remaps = config["Remaps"] as? [[String: Any]] ?? []
            remaps.removeAll { simpleTrigger($0, button, clicks: clicks, duration: duration) }
            if action != "default" {
                let effect: [String: Any] = action == "smartZoom" ? ["type": "smartZoom"] : ["type": "symbolicHotkey", "variant": Int(action.dropFirst(7)) ?? 32]
                remaps.append(["trigger": ["button": button, "level": clicks, "duration": duration], "modifiers": [:], "effect": effect])
            }
            config["Remaps"] = remaps
        }
    }
}

private func simpleTrigger(_ row: [String: Any], _ button: Int, clicks: Int = 1, duration: String = "click") -> Bool {
    guard let trigger = row["trigger"] as? [String: Any], let modifiers = row["modifiers"] as? [String: Any] else { return false }
    return trigger["button"] as? Int == button && trigger["level"] as? Int == clicks && trigger["duration"] as? String == duration && modifiers.isEmpty
}

// MMF's existing local message protocol reloads preferences without killing its helper.
private func notifyMouseFix() {
    guard let data = try? NSKeyedArchiver.archivedData(withRootObject: ["message": "configFileChanged"], requiringSecureCoding: false) else { return }
    for name in ["com.nuebling.mac-mouse-fix.helper", "com.nuebling.mac-mouse-fix"] {
        if let port = CFMessagePortCreateRemote(nil, name as CFString) {
            _ = CFMessagePortSendRequest(port, 0x420666, data as CFData, 0.2, 0, nil, nil)
        }
    }
}


// Read-only permission check; never changes remote-input ownership.
private func mouseEngineAccessibility() -> Bool? {
    guard let port = CFMessagePortCreateRemote(nil, "com.nuebling.mac-mouse-fix.helper" as CFString),
          let data = try? NSKeyedArchiver.archivedData(withRootObject: ["message": "checkAccessibility"], requiringSecureCoding: false) else { return nil }
    var reply: Unmanaged<CFData>?
    guard CFMessagePortSendRequest(port, 0x420666, data as CFData, 0.2, 0.2, CFRunLoopMode.defaultMode.rawValue, &reply) == 0,
          let reply else { return nil }
    return (try? NSKeyedUnarchiver.unarchivedObject(ofClass: NSNumber.self, from: reply.takeRetainedValue() as Data))?.boolValue
}

// The UI edits the engine's native rows; unknown effects and modifier combinations
// stay intact. Match the displayed snapshot when saving so a peer's newer edit
// cannot silently be overwritten by a stale popup.
struct MouseRemap {
    let value: [String: Any]
    var trigger: [String: Any] { value["trigger"] as? [String: Any] ?? [:] }
    var modifiers: [String: Any] { value["modifiers"] as? [String: Any] ?? [:] }
    var buttonModifiers: [[String: Any]] { modifiers["buttonModifiers"] as? [[String: Any]] ?? [] }
    var button: Int { trigger["button"] as? Int ?? buttonModifiers.last?["button"] as? Int ?? 0 }
    var kind: String { value["trigger"] as? String ?? "click" }
    var effect: [String: Any] { value["effect"] as? [String: Any] ?? [:] }
    func hasSameTrigger(as other: MouseRemap) -> Bool {
        NSDictionary(dictionary: modifiers).isEqual(to: other.modifiers) &&
        NSDictionary(dictionary: ["trigger": value["trigger"] ?? ""]).isEqual(to: ["trigger": other.value["trigger"] ?? ""])
    }
    var actions: [MouseAction] { MouseAction.options(for: kind) }
    var actionID: String { actions.first { NSDictionary(dictionary: $0.effect).isEqual(to: effect) }?.id ?? "custom" }
    var actionTitle: String {
        if let action = actions.first(where: { $0.id == actionID }) { return action.title }
        if effect["type"] as? String == "keyboardShortcut", let key = effect["keycode"] as? Int {
            return mouseModifierName(effect["flags"] as? UInt ?? 0) + mouseKeyName(key)
        }
        return "Custom Action"
    }
    var title: String {
        let level = trigger["level"] as? Int ?? buttonModifiers.last?["level"] as? Int ?? 1
        let click = level == 1 ? "Click" : level == 2 ? "Double Click" : "\(level)× Click"
        let action = kind == "dragTrigger" ? "\(click) and Drag" : kind == "scrollTrigger" ? "\(click) and Scroll" : trigger["duration"] as? String == "hold" ? "\(click) and Hold" : click
        let otherButtons = kind == "click" ? buttonModifiers : Array(buttonModifiers.dropLast())
        let prefix = otherButtons.map { "\(($0["level"] as? Int ?? 1) == 1 ? "Click" : "Double Click") \(mouseButtonName($0["button"] as? Int ?? 0)) + " }.joined()
        return mouseModifierName(modifiers["keyboardModifiers"] as? UInt ?? 0) + prefix + action
    }
    static func make(button: Int, gesture: String, flags: UInt = 0, effect: [String: Any]) -> MouseRemap {
        var modifiers: [String: Any] = flags == 0 ? [:] : ["keyboardModifiers": flags]
        let trigger: Any
        if ["dragTrigger", "scrollTrigger"].contains(gesture) {
            trigger = gesture; modifiers["buttonModifiers"] = [["button": button, "level": 1]]
        } else {
            trigger = ["button": button, "level": gesture == "double" ? 2 : 1, "duration": gesture == "hold" ? "hold" : "click"] as [String: Any]
        }
        return MouseRemap(value: ["trigger": trigger, "modifiers": modifiers, "effect": effect])
    }
}

struct MouseAction: Identifiable {
    let id: String
    let title: String
    let effect: [String: Any]
    static func options(for kind: String) -> [MouseAction] {
        if kind == "dragTrigger" {
            return [MouseAction(id: "pan", title: "Scroll & Navigate", effect: ["modifiedDragType": "twoFingerSwipe"]),
                    MouseAction(id: "spaces", title: "Spaces & Mission Control", effect: ["modifiedDragType": "threeFingerSwipe"]),
                    MouseAction(id: "zoom", title: "Zoom In or Out", effect: ["modifiedDragType": "zoom"])]
        }
        if kind == "scrollTrigger" {
            return [("fourFingerPinch", "Desktop & Launchpad"), ("threeFingerSwipeHorizontal", "Move Between Spaces"),
                    ("zoom", "Zoom In or Out"), ("horizontal", "Horizontal Scroll"), ("rotate", "Rotate")].map {
                MouseAction(id: $0.0, title: $0.1, effect: ["modifiedScrollEffectModification": $0.0])
            } + [("fast", "Swift Scroll"), ("precision", "Precise Scroll")].map {
                MouseAction(id: $0.0, title: $0.1, effect: ["modifiedScrollInputModification": $0.0])
            }
        }
        return [MouseAction(id: "lookUp", title: "Look Up & Quick Look", effect: ["type": "symbolicHotkey", "variant": 70]),
                MouseAction(id: "smartZoom", title: "Smart Zoom", effect: ["type": "smartZoom"])] +
            [(1, "Primary Click"), (2, "Secondary Click"), (3, "Middle Click")].map {
                MouseAction(id: "button:\($0.0)", title: $0.1, effect: ["type": "mouseButton", "button": $0.0, "nOfClicks": 1])
            } + [("left", "Back"), ("right", "Forward")].map {
                MouseAction(id: $0.0, title: $0.1, effect: ["type": "navigationSwipe", "variant": $0.0])
            } + [(32, "Mission Control"), (33, "Application Windows"), (36, "Show Desktop"), (160, "Launchpad"), (79, "Move Left a Space"), (81, "Move Right a Space")].map {
                MouseAction(id: "hotkey:\($0.0)", title: $0.1, effect: ["type": "symbolicHotkey", "variant": $0.0])
            } + [(3, "Brightness Down"), (2, "Brightness Up"), (20, "Previous Track"), (16, "Play / Pause"), (19, "Next Track"), (7, "Mute"), (1, "Volume Down"), (0, "Volume Up")].map {
                MouseAction(id: "media:\($0.0)", title: $0.1, effect: ["type": "systemDefinedEvent", "systemDefinedEventType": $0.0, "flags": 0])
            }
    }
}

func mouseButtonName(_ button: Int) -> String { button == 3 ? "Middle Button" : button == 0 ? "Other Actions" : "Button \(button)" }
func mouseModifierName(_ flags: UInt) -> String {
    [(NSEvent.ModifierFlags.control, "⌃"), (.option, "⌥"), (.shift, "⇧"), (.command, "⌘")]
        .filter { flags & $0.0.rawValue != 0 }.map { $0.1 }.joined()
}
func mouseKeyName(_ key: Int) -> String {
    let names = [0:"A",1:"S",2:"D",3:"F",4:"H",5:"G",6:"Z",7:"X",8:"C",9:"V",11:"B",12:"Q",13:"W",14:"E",15:"R",16:"Y",17:"T",18:"1",19:"2",20:"3",21:"4",22:"6",23:"5",24:"=",25:"9",26:"7",27:"−",28:"8",29:"0",30:"]",31:"O",32:"U",33:"[",34:"I",35:"P",36:"↩",37:"L",38:"J",39:"'",40:"K",41:";",42:"\\",43:",",44:"/",45:"N",46:"M",47:".",48:"⇥",49:"Space",50:"`",51:"⌫",53:"⎋",117:"⌦",123:"←",124:"→",125:"↓",126:"↑"]
    return names[key] ?? "Key \(key)"
}

func replaceMouseRemap(in config: inout [String: Any], old: MouseRemap?, new: MouseRemap?) throws {
    var rows = config["Remaps"] as? [[String: Any]] ?? []
    if let old {
        guard let index = rows.firstIndex(where: { NSDictionary(dictionary: $0).isEqual(to: old.value) }) else {
            throw NSError(domain: "MouseProfile", code: 2, userInfo: [NSLocalizedDescriptionKey: "This action changed on another Mac. Please try again with the updated settings."])
        }
        if let new { rows[index] = new.value } else { rows.remove(at: index) }
    } else if let new {
        guard !rows.contains(where: { MouseRemap(value: $0).hasSameTrigger(as: new) }) else {
            throw NSError(domain: "MouseProfile", code: 3, userInfo: [NSLocalizedDescriptionKey: "This gesture already has an action. Change it in the table instead."])
        }
        rows.append(new.value)
    }
    config["Remaps"] = rows
}

let defaultMouseScrollModifiers: [String: Any] = ["horizontal": UInt(1 << 17), "zoom": UInt(1 << 20), "swift": UInt(1 << 18), "precise": UInt(1 << 19)]
func setMouseScrollModifier(in config: inout [String: Any], key: String, flags: UInt) {
    var scroll = config["Scroll"] as? [String: Any] ?? [:]
    var modifiers = scroll["modifiers"] as? [String: Any] ?? [:]
    for other in ["horizontal", "zoom", "swift", "precise"] where other != key && modifiers[other] as? UInt == flags { modifiers[other] = UInt(0) }
    modifiers[key] = flags; scroll["modifiers"] = modifiers; config["Scroll"] = scroll
}

extension MouseProfileModel {
    var remaps: [MouseRemap] { (config["Remaps"] as? [[String: Any]] ?? []).map { MouseRemap(value: $0) } }
    func changeAction(_ row: MouseRemap, effect: [String: Any]) {
        var value = row.value; value["effect"] = effect
        edit { try replaceMouseRemap(in: &$0, old: row, new: MouseRemap(value: value)) }
    }
    func removeAction(_ row: MouseRemap) { edit { try replaceMouseRemap(in: &$0, old: row, new: nil) } }
    func addAction(_ row: MouseRemap) { edit { try replaceMouseRemap(in: &$0, old: nil, new: row) } }
    func restoreButtons(_ preset: String) {
        edit { config in
            guard let defaults = (config["Constants"] as? [String: Any])?["defaultRemaps"] as? [String: Any], let rows = defaults[preset] as? [[String: Any]] else { throw CocoaError(.propertyListReadCorrupt) }
            config["Remaps"] = rows
        }
    }
}
