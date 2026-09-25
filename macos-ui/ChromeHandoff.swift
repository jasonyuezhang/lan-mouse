import AppKit

let chromeBundleID = "com.google.Chrome"
/// Travels through the file bridge as a tiny file; the receiver opens it instead of dropping it.
let chromeHandoffExtension = "lanmouse-url"

struct ChromeHandoff: Codable, Equatable {
    let url: String
    let profile: String?

    /// Only web pages cross; `file:`, `javascript:` and `chrome:` URLs stay on this Mac.
    var webURL: URL? {
        guard let url = URL(string: url), ["http", "https"].contains(url.scheme?.lowercased() ?? ""), url.host?.isEmpty == false else { return nil }
        return url
    }
}

/// With several profiles Chrome titles windows "Page - Google Chrome - Person (Profile)",
/// or "Page - Google Chrome - Profile" when the person and profile names match.
func chromeProfileName(windowTitle: String) -> String? {
    guard let range = windowTitle.range(of: " - Google Chrome - ", options: .backwards) else { return nil }
    let suffix = windowTitle[range.upperBound...].trimmingCharacters(in: .whitespaces)
    if suffix.hasSuffix(")"), let open = suffix.range(of: " (", options: .backwards) {
        let name = suffix[open.upperBound..<suffix.index(before: suffix.endIndex)]
        if !name.isEmpty { return String(name) }
    }
    return suffix.isEmpty ? nil : suffix
}

/// Maps a profile's display name to its directory using Chrome's `Local State` JSON.
func chromeProfileDirectory(named name: String, localState: Data) -> String? {
    guard let root = try? JSONSerialization.jsonObject(with: localState) as? [String: Any],
          let cache = (root["profile"] as? [String: Any])?["info_cache"] as? [String: Any] else { return nil }
    return cache.first { ($0.value as? [String: Any])?["name"] as? String == name }?.key
}

private func axValue(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

/// Chrome's focused window, read through Accessibility.
struct ChromeWindow {
    let element: AXUIElement
    let title: String
    let position: CGPoint
    var document: String? { axValue(element, "AXDocument") as? String }
    var size: CGSize {
        var size = CGSize.zero
        if let raw = axValue(element, "AXSize"), CFGetTypeID(raw) == AXValueGetTypeID() { AXValueGetValue(raw as! AXValue, .cgSize, &size) }
        return size
    }

    static func focused(pid: pid_t) -> ChromeWindow? {
        let app = AXUIElementCreateApplication(pid)
        guard let value = axValue(app, "AXFocusedWindow"), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        let window = value as! AXUIElement
        var position = CGPoint.zero
        if let raw = axValue(window, "AXPosition"), CFGetTypeID(raw) == AXValueGetTypeID() { AXValueGetValue(raw as! AXValue, .cgPoint, &position) }
        return ChromeWindow(element: window, title: axValue(window, "AXTitle") as? String ?? "", position: position)
    }

    static func all(pid: pid_t) -> [ChromeWindow] {
        (axValue(AXUIElementCreateApplication(pid), "AXWindows") as? [AXUIElement] ?? []).map {
            ChromeWindow(element: $0, title: axValue($0, "AXTitle") as? String ?? "", position: .zero)
        }
    }

    /// Counts tab-strip buttons, never descending into page content.
    var tabCount: Int {
        var count = 0, queue = [(element, 0)]
        while let (item, depth) = queue.popLast() {
            let role = axValue(item, "AXRole") as? String
            if role == "AXWebArea" { continue }
            if role == "AXRadioButton", axValue(item, "AXSubrole") as? String == "AXTabButton" { count += 1; continue }
            if depth < 14, let children = axValue(item, "AXChildren") as? [AXUIElement] { queue += children.map { ($0, depth + 1) } }
        }
        return count
    }
}

/// Opens a received page in the same-named profile when this Mac has one.
@MainActor func openChromeHandoff(_ handoff: ChromeHandoff) -> String {
    guard let url = handoff.webURL else { return "Ignored a Chrome page that isn’t a web address." }
    guard let chrome = NSWorkspace.shared.urlForApplication(withBundleIdentifier: chromeBundleID) else {
        NSWorkspace.shared.open(url)
        return "Chrome isn’t installed here, so the page opened in the default browser."
    }
    let localState = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Google/Chrome/Local State")
    let label = "Opened \(url.host ?? "page") in Chrome" + (handoff.profile.map { " (\($0))" } ?? "") + "."
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = true
    if let name = handoff.profile, let state = try? Data(contentsOf: localState), let directory = chromeProfileDirectory(named: name, localState: state) {
        // Like `open -na`: the new process hands its command line to the running Chrome.
        configuration.createsNewApplicationInstance = true
        configuration.arguments = ["--profile-directory=\(directory)", url.absoluteString]
        NSWorkspace.shared.openApplication(at: chrome, configuration: configuration)
        return label
    }
    if let running = NSRunningApplication.runningApplications(withBundleIdentifier: chromeBundleID).first,
       let window = ChromeWindow.all(pid: running.processIdentifier).first(where: { chromeProfileName(windowTitle: $0.title) == handoff.profile }) {
        // Chrome's profile list is unreadable (macOS app-data privacy). Chrome opens
        // external links in its most recently active profile, so focus one of its windows.
        AXUIElementPerformAction(window.element, kAXRaiseAction as CFString)
        AXUIElementSetAttributeValue(window.element, kAXMainAttribute as CFString, kCFBooleanTrue)
        running.activate()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { NSWorkspace.shared.open([url], withApplicationAt: chrome, configuration: configuration) }
        return label
    }
    NSWorkspace.shared.open([url], withApplicationAt: chrome, configuration: configuration)
    return handoff.profile == nil ? label : "Opened \(url.host ?? "page") in Chrome’s last-used profile; no “\(handoff.profile!)” window is open here."
}
