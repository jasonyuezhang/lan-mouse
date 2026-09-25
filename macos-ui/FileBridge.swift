import AppKit
import SwiftUI

private let fileBridgeRoot = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config/lan-mouse/file-bridge", isDirectory: true)

struct FileBridgeJob: Codable {
    let id: String
    let peer: String
    let source: String
    let created: Double
    var activate = false
}
struct ReceivedDrag: Decodable {
    let id: String
    let peer: String
    let path: String
    let created: Double
}

func shouldPrepareFileDrag(position: String, mouse: NSPoint, previous: NSPoint?, screen: NSRect) -> Bool {
    let distance: CGFloat
    let approaching: Bool
    switch position {
    case "left": distance = mouse.x - screen.minX; approaching = previous.map { mouse.x < $0.x } ?? false
    case "right": distance = screen.maxX - mouse.x; approaching = previous.map { mouse.x > $0.x } ?? false
    case "top": distance = screen.maxY - mouse.y; approaching = previous.map { mouse.y > $0.y } ?? false
    case "bottom": distance = mouse.y - screen.minY; approaching = previous.map { mouse.y < $0.y } ?? false
    default: return false
    }
    // Copy only near a shared edge; ordinary drags elsewhere stay on this Mac.
    return distance < 30 || (distance < 240 && approaching)
}

@MainActor final class FileBridge: ObservableObject {
    @Published private(set) var enabled = false
    @Published private(set) var status = "Drag a file across to Slack, Orca, or another app that accepts files."
    private weak var model: ControlModel?
    private var timer: Timer?
    private var boardChange = NSPasteboard(name: .drag).changeCount
    private var armed: (url: URL, pid: pid_t)?
    /// Set when `armed` is a Chrome page rather than a file.
    private var chrome: ChromeHandoff?
    /// Chrome's focused window when the current press was first seen; nil window once ruled out.
    private var chromePress: (window: AXUIElement?, position: CGPoint, size: CGSize)?
    private var previousDragPoint: NSPoint?
    private var job: FileBridgeJob?
    private var peerID: Int?
    private var ready = false
    private var crossed = false
    private var scans = 0
    private var seen = Set<String>()
    private var relay: FileDragRelay?
    private var hud: NSPanel?
    private var hudState: FileDragHUDState?
    private var inputActivity: NSObjectProtocol?

    func start(_ model: ControlModel) {
        guard timer == nil else { return }
        self.model = model
        enabled = FileManager.default.fileExists(atPath: fileBridgeRoot.appendingPathComponent("enabled").path)
        try? FileManager.default.removeItem(at: fileBridgeRoot.appendingPathComponent("outgoing-pages"))
        connectionChanged()
        // Old transfers remain readable by agents, but are never replayed after restart.
        seen = Set((try? FileManager.default.contentsOfDirectory(atPath: fileBridgeRoot.appendingPathComponent("received").path)) ?? [])
        let timer = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in MainActor.assumeIsolated { self?.tick() } }
        RunLoop.main.add(timer, forMode: .common); self.timer = timer
    }
    func setEnabled(_ value: Bool) {
        do {
            try FileManager.default.createDirectory(at: fileBridgeRoot.appendingPathComponent("outgoing"), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: fileBridgeRoot.path)
            let flag = fileBridgeRoot.appendingPathComponent("enabled")
            if value { try Data().write(to: flag, options: .atomic) }
            else { if FileManager.default.fileExists(atPath: flag.path) { try FileManager.default.removeItem(at: flag) }; cancel(); relay?.cancel() }
            enabled = value
            connectionChanged()
        } catch { status = error.localizedDescription }
    }
    func connectionChanged() {
        let listening = enabled && model?.connected == true
        if !listening { relay?.cancel() }
        if listening, inputActivity == nil {
            // Keep the receiver's handoff timer responsive with its window closed.
            // The Mac may still sleep normally when the user leaves the desk.
            inputActivity = ProcessInfo.processInfo.beginActivity(options: .userInitiatedAllowingIdleSystemSleep, reason: "Receiving shared mouse and file drags")
        } else if !listening, let activity = inputActivity {
            ProcessInfo.processInfo.endActivity(activity)
            inputActivity = nil
        }
    }
    private func jobURL(_ id: String) -> URL { fileBridgeRoot.appendingPathComponent("outgoing/\(id).json") }
    private func saveJob() throws {
        guard let job else { return }
        try JSONEncoder().encode(job).write(to: jobURL(job.id), options: .atomic)
    }
    private func cancel() {
        if let job, !crossed { try? FileManager.default.removeItem(at: jobURL(job.id)) }
        model?.send(["SetFileDragReady": false], record: false)
        if chrome != nil, let armed { try? FileManager.default.removeItem(at: armed.url.deletingLastPathComponent()) }
        job = nil; peerID = nil; armed = nil; chrome = nil; previousDragPoint = nil; ready = false; crossed = false
        hideHUD()
    }
    private func tick() {
        guard enabled, model?.connected == true else { if armed != nil { cancel() }; return }
        scans += 1
        if scans % 5 == 0 { scanReceived() }
        guard relay == nil else { return }
        let board = NSPasteboard(name: .drag)
        let held = NSEvent.pressedMouseButtons & 1 != 0
        if board.changeCount != boardChange {
            boardChange = board.changeCount
            if held, !crossed,
               let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
                cancel()
                guard urls.count == 1 else { status = "Drag one file at a time."; return }
                let url = urls[0]
                // The daemon validates and reads the selected file near the edge.
                // Probing it here would require a second app's folder permission.
                armed = (url, NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0)
            } else if held, !crossed, let chromeApp = frontmostChrome(),
                      let link = board.string(forType: .URL), ChromeHandoff(url: link, profile: nil).webURL != nil {
                // A link or address-bar drag out of Chrome.
                cancel()
                armChrome(ChromeHandoff(url: link, profile: ChromeWindow.focused(pid: chromeApp.processIdentifier).flatMap { chromeProfileName(windowTitle: $0.title) }), pid: chromeApp.processIdentifier)
            }
        }
        if !held { chromePress = nil }
        if held, !crossed, armed == nil, let chromeApp = frontmostChrome() { detectChromeTabDrag(pid: chromeApp.processIdentifier) }
        if !held && !crossed { if armed != nil { cancel() }; return }
        guard let armed else { return }
        if job == nil, let model {
            let mouse = NSEvent.mouseLocation
            let previous = previousDragPoint
            previousDragPoint = mouse
            guard let screen = NSScreen.screens.first(where: { $0.frame.insetBy(dx: -1, dy: -1).contains(mouse) }) else { return }
            guard let peer = model.peers.first(where: { $0.active && shouldPrepareFileDrag(position: $0.position, mouse: mouse, previous: previous, screen: screen.frame) }),
                  let fingerprint = model.verifiedIdentity(for: peer) else { return }
            model.send(["PrepareFileDrag": peer.id], record: false)
            peerID = peer.id
            job = FileBridgeJob(id: UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased(), peer: fingerprint, source: armed.url.path, created: Date().timeIntervalSince1970)
            do { try saveJob(); status = "Preparing \(armedName)…"; showHUD(file: armedName, position: peer.position) }
            catch { status = "Couldn’t prepare the file: \(error.localizedDescription)"; cancel() }
        }
        guard let job else { return }
        // macOS may refresh its Local Network permission cache after an update.
        // Retry the lazy connection while the user is holding the file at the edge.
        if !crossed, !ready, scans % 20 == 0, let peerID {
            model?.send(["PrepareFileDrag": peerID], record: false)
        }
        if Date().timeIntervalSince1970 - job.created > 120 { status = "Transfer timed out. Keep sharing and Lan Mouse Control on at both Macs."; cancel(); return }
        let stateURL = fileBridgeRoot.appendingPathComponent("outgoing/\(job.id).status")
        if let data = try? Data(contentsOf: stateURL), let state = try? JSONSerialization.jsonObject(with: data) as? [String: Any], let phase = state["state"] as? String {
            if phase == "ready" && !crossed {
                if !ready { model?.send(["SetFileDragReady": true], record: false) }
                ready = true; status = "Ready — continue across and drop into your app."; showHUD(file: armedName, isReady: true)
            } else if phase == "sending" && !crossed {
                let percent = Int((state["progress"] as? Double ?? 0) * 100)
                status = "Copying \(armedName) · \(percent)%"; showHUD(file: armedName, progress: Double(percent) / 100)
            } else if phase.hasPrefix("error:") { status = String(phase.dropFirst(7)); cancel(); return }
            else if phase == "activated" { status = chrome == nil ? "File handed to the other Mac. Drop it into your app." : "Opened \(armedName) on the other Mac."; cancel(); return }
        }
        if ready && !crossed && scans % 4 == 0 { model?.send(["SetFileDragReady": true], record: false) }
    }
    private func frontmostChrome() -> NSRunningApplication? {
        let app = NSWorkspace.shared.frontmostApplication
        return app?.bundleIdentifier == chromeBundleID ? app : nil
    }
    /// A tab pulled out of Chrome becomes a one-tab window that follows the pointer.
    /// Dragging a one-tab window by its title bar counts too; resizing does not.
    private func detectChromeTabDrag(pid: pid_t) {
        guard AXIsProcessTrusted(), let window = ChromeWindow.focused(pid: pid) else { return }
        let size = window.size
        guard let press = chromePress, let pressed = press.window, CFEqual(pressed, window.element) else {
            chromePress = (window.element, window.position, size); return
        }
        guard hypot(window.position.x - press.position.x, window.position.y - press.position.y) > 3, size == press.size else { return }
        chromePress = (nil, window.position, size) // decide once per press
        guard window.tabCount == 1, let url = window.document else { return }
        armChrome(ChromeHandoff(url: url, profile: chromeProfileName(windowTitle: window.title)), pid: pid)
    }
    private func armChrome(_ handoff: ChromeHandoff, pid: pid_t) {
        guard let page = handoff.webURL else { return }
        let dir = fileBridgeRoot.appendingPathComponent("outgoing-pages/\(UUID().uuidString)", isDirectory: true)
        let file = dir.appendingPathComponent("\(page.host ?? "page").\(chromeHandoffExtension)")
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try JSONEncoder().encode(handoff).write(to: file, options: .atomic)
            armed = (file, pid); chrome = handoff
        } catch { status = "Couldn’t prepare the Chrome page: \(error.localizedDescription)" }
    }
    private var armedName: String {
        guard let armed else { return "" }
        guard let chrome else { return armed.url.lastPathComponent }
        return (chrome.webURL?.host ?? "Chrome page") + (chrome.profile.map { " · \($0)" } ?? "")
    }
    func captured(handle: Int) {
        relay?.cancel()
        guard enabled, ready, !crossed, peerID == handle, NSEvent.pressedMouseButtons & 1 != 0, let armed, job != nil else { return }
        crossed = true; ready = false; job?.activate = true
        if chrome != nil {
            // The page opens over there; Escape puts the dragged tab back where it was.
            model?.send(["CancelNativeFileDrag": ["pid": armed.pid]], record: false)
            do { try saveJob() } catch { status = error.localizedDescription }
            model?.send(["SetFileDragReady": false], record: false)
            hideHUD(); return
        }
        // A file drag began before capture, so the receiver has not seen its mouse-down.
        model?.send(["StartFileDragFrom": ["handle": handle, "source_pid": armed.pid]], record: false)
        do { try saveJob() } catch { status = error.localizedDescription }
        model?.send(["SetFileDragReady": false], record: false)
        hideHUD()
    }
    private func scanReceived() {
        guard relay == nil, let model else { return }
        let root = fileBridgeRoot.appendingPathComponent("received")
        for id in (try? FileManager.default.contentsOfDirectory(atPath: root.path)) ?? [] where !seen.contains(id) {
            let dir = root.appendingPathComponent(id)
            guard let stamp = try? String(contentsOf: dir.appendingPathComponent("activate"), encoding: .utf8), let time = Double(stamp) else { continue }
            seen.insert(id)
            let age = Date().timeIntervalSince1970 - time
            NSLog("File bridge: activation %@ age %.3f, authorized peers %d", id, age, model.authorized.count)
            guard age < 5 else {
                status = "Drag handoff arrived too late. Please try again."
                continue
            }
            guard let data = try? Data(contentsOf: dir.appendingPathComponent("ready.json")),
                  let received = try? JSONDecoder().decode(ReceivedDrag.self, from: data),
                  received.id == id, model.authorized[received.peer] != nil else {
                status = "Couldn’t verify the received file’s paired Mac."
                NSLog("File bridge: rejected activation metadata or trust %@", id)
                continue
            }
            let url = URL(fileURLWithPath: received.path)
            guard url.deletingLastPathComponent().standardizedFileURL == dir.appendingPathComponent("files").standardizedFileURL,
                  FileManager.default.fileExists(atPath: url.path) else {
                status = "Couldn’t locate the received file."
                NSLog("File bridge: rejected received file location %@", id)
                continue
            }
            if url.pathExtension == chromeHandoffExtension {
                if let data = try? Data(contentsOf: url), data.count < 16_384, let handoff = try? JSONDecoder().decode(ChromeHandoff.self, from: data) {
                    status = openChromeHandoff(handoff)
                } else { status = "Couldn’t read the Chrome page from the other Mac." }
                try? Data("copied".utf8).write(to: dir.appendingPathComponent("handoff"), options: .atomic)
                continue
            }
            status = "Drop \(url.lastPathComponent) into your app."
            let relay = FileDragRelay(url: url, send: { [weak model] request in
                model?.send(request, record: false)
            }, started: {
                try? Data("dragging".utf8).write(to: dir.appendingPathComponent("handoff"), options: .atomic)
            }) { [weak self] copied in
                try? Data((copied ? "copied" : "cancelled").utf8).write(to: dir.appendingPathComponent("handoff"), options: .atomic)
                self?.relay = nil
                self?.boardChange = NSPasteboard(name: .drag).changeCount
                self?.status = copied ? "File dropped. A local copy stays available on this Mac." : "Drop cancelled. The received file is still available."
            }
            try? Data("starting".utf8).write(to: dir.appendingPathComponent("handoff"), options: .atomic)
            self.relay = relay; relay.begin(); break
        }
    }
    private func showHUD(file: String, position: String? = nil, progress: Double? = nil, isReady: Bool = false) {
        let presentation = FileDragHUDPresentation(file: file, position: position ?? hudState?.presentation.position ?? "right", progress: progress, isReady: isReady)
        if let hudState {
            if hudState.presentation != presentation { hudState.presentation = presentation }
            return
        }
        let state = FileDragHUDState(presentation)
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first(where: { $0.frame.insetBy(dx: -1, dy: -1).contains(mouse) }) ?? NSScreen.main
        let bounds = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 800, height: 600)
        let size = NSSize(width: 280, height: 76)
        let rect = NSRect(x: min(max(mouse.x + 20, bounds.minX + 12), bounds.maxX - size.width - 12), y: min(max(mouse.y - size.height - 20, bounds.minY + 12), bounds.maxY - size.height - 12), width: size.width, height: size.height)
        let panel = NSPanel(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .statusBar; panel.isFloatingPanel = true; panel.ignoresMouseEvents = true; panel.hidesOnDeactivate = false
        panel.isOpaque = false; panel.backgroundColor = .clear; panel.hasShadow = true
        panel.contentView = NSHostingView(rootView: FileDragHUDView(state: state))
        hud = panel; hudState = state
        panel.alphaValue = 0; panel.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : 0.16
            panel.animator().alphaValue = 1
        }
    }
    private func hideHUD() {
        guard let panel = hud else { return }
        hud = nil; hudState = nil
        // This visual fade never holds up input capture or the native handoff.
        NSAnimationContext.runAnimationGroup { context in
            context.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : 0.12
            panel.animator().alphaValue = 0
        } completionHandler: { panel.close() }
    }
}

struct FileDragHUDPresentation: Equatable {
    let file: String
    let position: String
    var progress: Double?
    var isReady = false
    var direction: String {
        switch position { case "left": return "left"; case "top": return "up"; case "bottom": return "down"; default: return "right" }
    }
    var title: String { isReady ? "Ready to cross" : progress == nil ? "Preparing file…" : "Copying file…" }
}
@MainActor final class FileDragHUDState: ObservableObject {
    @Published var presentation: FileDragHUDPresentation
    init(_ presentation: FileDragHUDPresentation) { self.presentation = presentation }
}
struct FileDragHUDView: View {
    @ObservedObject var state: FileDragHUDState
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
    @Environment(\.colorSchemeContrast) private var contrast
    var body: some View {
        let value = state.presentation
        HStack(spacing: 12) {
            ZStack {
                if !value.isReady {
                    Group {
                        if let progress = value.progress {
                            ProgressView(value: min(max(progress, 0), 1)).progressViewStyle(.circular)
                        } else { ProgressView().progressViewStyle(.circular) }
                    }
                    .controlSize(.small)
                    .transition(.opacity)
                }
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 28, weight: .regular))
                    .foregroundStyle(.green)
                    .opacity(value.isReady ? 1 : 0)
                    .scaleEffect(reduceMotion || value.isReady ? 1 : 0.85)
            }
            .frame(width: 30, height: 32)
            .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(value.title).font(.system(size: 13, weight: .semibold)).contentTransition(.opacity)
                Text(value.file).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            ZStack {
                if value.isReady {
                    Image(systemName: "arrow." + value.direction).font(.system(size: 16, weight: .semibold)).foregroundStyle(.secondary)
                } else if let progress = value.progress {
                    Text("\(Int((min(max(progress, 0), 1) * 100).rounded()))%")
                        .font(.system(size: 10, weight: .medium)).monospacedDigit().foregroundStyle(.secondary)
                }
            }.frame(width: 32).accessibilityHidden(true)
        }
        .padding(.horizontal, 16)
        .frame(width: 280, height: 76)
        .background {
            let shape = RoundedRectangle(cornerRadius: 18, style: .continuous)
            if reduceTransparency { shape.fill(Color(nsColor: .windowBackgroundColor)) }
            else { shape.fill(.regularMaterial) }
        }
        .overlay { RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(.primary.opacity(contrast == .increased ? 0.3 : 0.1), lineWidth: 0.5) }
        .animation(reduceMotion ? nil : .easeOut(duration: 0.2), value: value.isReady)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(value.file). \(value.title)" + (value.isReady ? ". Keep dragging \(value.direction)." : value.progress.map { ". \(Int($0 * 100)) percent." } ?? ""))
    }
}

@MainActor final class FileDragRelay: NSView, NSDraggingSource {
    private let file: URL
    private let started: () -> Void
    private let finished: (Bool) -> Void
    private let send: ([String: Any]) -> Void
    private var panel: NSPanel?
    private var previousApplication: NSRunningApplication?
    private var dragging = false
    private var ended = false
    private var cancelling = false
    private var mouseUpMonitors: [Any] = []
    private var timeout: Timer?
    init(url: URL, send: @escaping ([String: Any]) -> Void, started: @escaping () -> Void = {}, finished: @escaping (Bool) -> Void) {
        file = url; self.send = send; self.started = started; self.finished = finished
        super.init(frame: NSRect(x: 0, y: 0, width: 64, height: 64))
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    func begin() {
        NSLog("File bridge: beginning native drag, buttons %lu", NSEvent.pressedMouseButtons)
        let point = NSEvent.mouseLocation
        previousApplication = NSWorkspace.shared.frontmostApplication
        let panel = FileDragPanel(contentRect: NSRect(x: point.x - 16, y: point.y - 16, width: 64, height: 64), styleMask: [.borderless], backing: .buffered, defer: false)
        panel.isOpaque = false; panel.backgroundColor = NSColor.black.withAlphaComponent(0.01); panel.hasShadow = false; panel.level = .statusBar;panel.hidesOnDeactivate = false
        panel.contentView = self; self.panel = panel
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        // AppKit can constrain the panel away from a screen/menu-bar edge.
        // Keep the real press inside its actual frame, never on an app below it.
        let bounds = panel.frame.insetBy(dx: 2, dy: 2)
        let press = NSPoint(x: min(max(point.x, bounds.minX), bounds.maxX), y: min(max(point.y, bounds.minY), bounds.maxY))
        let cgPoint = NSPoint(x: press.x, y: (NSScreen.screens.first?.frame.maxY ?? 0) - press.y)
        if let monitor = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseUp, handler: { [weak self] _ in
            DispatchQueue.main.async { self?.released() }
        }) { mouseUpMonitors.append(monitor) }
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseUp, handler: { [weak self] event in
            self?.released(); return event
        }) { mouseUpMonitors.append(monitor) }
        let request: [String: Any] = ["BeginNativeFileDrag": ["pid": ProcessInfo.processInfo.processIdentifier, "window": panel.windowNumber, "x": Int(cgPoint.x), "y": Int(cgPoint.y)]]
        send(request)
        let deadline = Date().addingTimeInterval(120)
        let startupDeadline = Date().addingTimeInterval(2)
        let timer = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if Date() > deadline || (!self.dragging && Date() > startupDeadline) { self.cancel() }
                else if !self.dragging, !self.cancelling { self.send(request) }
            }
        }
        RunLoop.main.add(timer, forMode: .common); timeout = timer
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {
        guard !dragging, !ended, !cancelling else { return }
        dragging = true
        let icon = NSWorkspace.shared.icon(forFile: file.path); icon.size = NSSize(width: 32, height: 32)
        let item = NSDraggingItem(pasteboardWriter: file as NSURL)
        item.setDraggingFrame(NSRect(x: 0, y: 0, width: 32, height: 32), contents: icon)
        let session = beginDraggingSession(with: [item], event: event, source: self)
        session.animatesToStartingPositionsOnCancelOrFail = false
    }
    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation { .copy }
    func ignoreModifierKeys(for session: NSDraggingSession) -> Bool { true }
    func draggingSession(_ session: NSDraggingSession, willBeginAt screenPoint: NSPoint) {
        NSLog("File bridge: native drag began")
        // Tracking is established. Let the app beneath this temporary source
        // window receive drops, including ones close to the handoff point.
        panel?.ignoresMouseEvents = true
        started()
    }
    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) { finish(operation.contains(.copy)) }
    private func released() {
        // Give AppKit the release first. If it never calls endedAt, cancel the
        // abandoned native session so its file image cannot remain on screen.
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in self?.cancel() }
    }
    func cancel() {
        guard !ended, !cancelling else { return }
        cancelling = true
        send(["CancelNativeFileDrag": ["pid": ProcessInfo.processInfo.processIdentifier]])
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in self?.finish(false) }
    }
    private func finish(_ copied: Bool) {
        guard !ended else { return }
        ended = true
        timeout?.invalidate(); timeout = nil
        mouseUpMonitors.forEach(NSEvent.removeMonitor); mouseUpMonitors.removeAll()
        NSLog("File bridge: native drag finished, copied %d", copied ? 1 : 0)
        panel?.orderOut(nil); panel?.contentView = nil; panel = nil; finished(copied)
        if NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier {
            previousApplication?.activate(options: [])
        }
    }
}

private final class FileDragPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

struct FileBridgeSettings: View {
    @ObservedObject var bridge: FileBridge
    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 6) {
                Toggle("Drag files between Macs", isOn: Binding(get: { bridge.enabled }, set: bridge.setEnabled))
                Text(bridge.status).font(.caption).foregroundStyle(.secondary)
                if bridge.enabled { Text("Hold one file at the shared screen edge until Ready, then drop into Slack’s message field, Orca’s chat, or another file drop area. Up to 64 MB; copies stay on the receiving Mac. You send chat messages yourself.").font(.caption).foregroundStyle(.secondary) }
                if bridge.enabled {
                    Text("Chrome: drag a tab out of its window (or drag a link) to the edge until Ready, then keep going. It opens in Chrome on the other Mac, in the profile with the same name; the tab stays here.").font(.caption).foregroundStyle(.secondary)
                    if !AXIsProcessTrusted() {
                        Button("Allow Accessibility to hand off Chrome tabs…") {
                            _ = AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
                        }.font(.caption)
                    }
                }
            }.frame(maxWidth: .infinity, alignment: .leading).padding(6)
        }
    }
}
