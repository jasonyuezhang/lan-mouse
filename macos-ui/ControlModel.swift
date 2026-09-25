import Foundation
import AppKit
import Combine
import Darwin

struct Peer: Identifiable, Equatable {
    let id: Int
    var name: String
    var position: String
    var port: Int
    var active: Bool
    var alive: Bool
    var address: String?
    var ips: [String]
    var mappings: [String: Int]
    var version: String
    var pressed: Bool

    init?(_ row: Any) {
        guard let row = row as? [Any], row.count == 3,
              let id = row[0] as? Int, let config = row[1] as? [String: Any],
              let state = row[2] as? [String: Any] else { return nil }
        self.id = id
        ips = config["fix_ips"] as? [String] ?? []
        address = state["active_addr"] as? String
        name = config["hostname"] as? String ?? ips.last ?? "Computer \(id + 1)"
        port = config["port"] as? Int ?? 4242
        position = config["pos"] as? String ?? "left"
        active = state["active"] as? Bool ?? false
        alive = state["alive"] as? Bool ?? false
        mappings = config["key_map"] as? [String: Int] ?? [:]
        version = String(bytes: state["peer_commit"] as? [UInt8] ?? [], encoding: .utf8) ?? ""
        pressed = state["has_pressed_keys"] as? Bool ?? false
    }

    var identityKey: String { "\(name.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "."))):\(port)" }
    func matches(host: String, port: Int) -> Bool {
        self.port == port && (name.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: ".")) == host.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: ".")) || ips.contains(host))
    }

    var target: String? {
        if let address {
            if address.hasPrefix("["), let end = address.firstIndex(of: "]") {
                return String(address[address.index(after: address.startIndex)..<end])
            }
            if let colon = address.lastIndex(of: ":") { return String(address[..<colon]) }
        }
        return ips.last
    }
}

struct MappingRow: Identifiable {
    var id = UUID()
    var from: Int
    var to: Int
}

// A draft belongs to one computer, independently of selection on other pages.
struct MappingDraft {
    var peerID: Int?
    var loaded: [String: Int] = [:]
    var rows: [MappingRow] = []
    var dictionary: [String: Int] { Dictionary(rows.map { (String($0.from), $0.to) }, uniquingKeysWith: { _, new in new }) }
    var duplicate: Bool { Set(rows.map(\.from)).count != rows.count }
    var dirty: Bool { duplicate || dictionary != loaded }
    func matchesSaved(_ peer: Peer) -> Bool { peerID == peer.id && !duplicate && dictionary == peer.mappings }
    mutating func load(_ peer: Peer) {
        peerID = peer.id; loaded = peer.mappings
        rows = peer.mappings.compactMap { key, value in Int(key).map { MappingRow(from: $0, to: value) } }.sorted { $0.from < $1.from }
    }
}

struct Sample: Identifiable {
    let id = UUID()
    let date = Date()
    let ms: Double?
}

struct TraceLine: Identifiable {
    let id = UUID()
    let date = Date()
    let text: String
}

struct KeyChoice: Codable, Identifiable, Hashable {
    let id: Int
    let name: String
    static let all: [KeyChoice] = {
        guard let url = Bundle.main.url(forResource: "Keys", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let keys = try? JSONDecoder().decode([KeyChoice].self, from: data) else { return [] }
        return keys
    }()
    static let buttons = [KeyChoice(id: 272, name: "Left click"), KeyChoice(id: 273, name: "Right click"),
                          KeyChoice(id: 274, name: "Middle click"), KeyChoice(id: 275, name: "Back"),
                          KeyChoice(id: 276, name: "Forward")]
    static func label(_ id: Int) -> String { (all + buttons).first { $0.id == id }?.name ?? "Code \(id)" }
}

// The daemon already exposes newline-delimited JSON over its local Unix socket.
// No HTTP server, remote shell, or additional permissions are needed by this app.
final class IPCLink: @unchecked Sendable {
    private let lock = NSLock()
    private let writer = DispatchQueue(label: "lan-mouse.ipc-writer", qos: .userInitiated)
    private var fd: Int32 = -1
    let event: (Data) -> Void
    let status: (Bool) -> Void

    init(event: @escaping (Data) -> Void, status: @escaping (Bool) -> Void) {
        self.event = event; self.status = status
    }
    func start() {
        DispatchQueue.global(qos: .utility).async { [self] in
            while true {
                let socketFD = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
                guard socketFD >= 0 else { Thread.sleep(forTimeInterval: 1); continue }
                var address = sockaddr_un()
                address.sun_family = sa_family_t(AF_UNIX)
                let path = NSHomeDirectory() + "/Library/Caches/lan-mouse-socket.sock"
                guard path.utf8.count < MemoryLayout.size(ofValue: address.sun_path) else {
                    Darwin.close(socketFD); status(false); return
                }
                withUnsafeMutablePointer(to: &address.sun_path) {
                    $0.withMemoryRebound(to: CChar.self, capacity: 104) { _ = strcpy($0, path) }
                }
                let result = withUnsafePointer(to: &address) {
                    $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                        Darwin.connect(socketFD, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                    }
                }
                guard result == 0 else { Darwin.close(socketFD); Thread.sleep(forTimeInterval: 1); continue }
                var one: Int32 = 1
                setsockopt(socketFD, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout.size(ofValue: one)))
                var timeout = timeval(tv_sec: 1, tv_usec: 0)
                setsockopt(socketFD, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout.size(ofValue: timeout)))
                lock.lock(); fd = socketFD; lock.unlock()
                status(true)
                send("Sync")
                var pending = Data()
                var bytes = [UInt8](repeating: 0, count: 16384)
                while true {
                    let n = Darwin.read(socketFD, &bytes, bytes.count)
                    if n <= 0 { break }
                    pending.append(contentsOf: bytes.prefix(n))
                    if pending.count > 1_048_576 { break }
                    while let newline = pending.firstIndex(of: 10) {
                        event(Data(pending[..<newline])); pending.removeSubrange(...newline)
                    }
                }
                lock.lock(); fd = -1; Darwin.close(socketFD); lock.unlock()
                status(false)
                Thread.sleep(forTimeInterval: 1)
            }
        }
    }
    func send(_ message: Any, completion: @escaping (Bool) -> Void = { _ in }) {
        guard var data = try? JSONSerialization.data(withJSONObject: message, options: [.fragmentsAllowed]) else { completion(false); return }
        data.append(10)
        let payload = data
        writer.async { [self] in
            lock.lock()
            let success = fd >= 0 && payload.withUnsafeBytes { buffer in
                var offset = 0
                while offset < buffer.count {
                    let n = Darwin.write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                    if n < 0 && errno == EINTR { continue }
                    if n <= 0 { return false }; offset += n
                }
                return true
            }
            if !success && fd >= 0 { shutdown(fd, SHUT_RDWR) }
            lock.unlock()
            completion(success)
        }
    }

}

struct CommandResult { let code: Int32; let text: String }
func command(_ executable: String, _ arguments: [String], timeout: Double = 3) async -> CommandResult {
    await withCheckedContinuation { continuation in
        DispatchQueue.global(qos: .utility).async {
            let process = Process(), pipe = Pipe(), finished = DispatchSemaphore(value: 0)
            process.executableURL = URL(fileURLWithPath: executable); process.arguments = arguments
            process.standardOutput = pipe; process.standardError = pipe
            process.terminationHandler = { _ in finished.signal() }
            do { try process.run() } catch {
                continuation.resume(returning: CommandResult(code: -1, text: error.localizedDescription)); return
            }
            // Process.waitUntilExit can stall when a Swift task resumes on a different
            // thread. Use its termination callback, with a bounded timeout instead.
            var timedOut = false
            if finished.wait(timeout: .now() + timeout) == .timedOut {
                timedOut = true; process.terminate()
                if finished.wait(timeout: .now() + 0.5) == .timedOut {
                    kill(process.processIdentifier, SIGKILL)
                    if finished.wait(timeout: .now() + 1) == .timedOut {
                        continuation.resume(returning: CommandResult(code: -1, text: "Command did not exit: \(executable)")); return
                    }
                }
            }
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            continuation.resume(returning: CommandResult(code: timedOut ? -1 : process.terminationStatus,
                text: String(decoding: data, as: UTF8.self)))
        }
    }
}

func pingTime(_ text: String) -> Double? {
    guard let range = text.range(of: #"time[=<]([0-9.]+)\s*ms"#, options: .regularExpression) else { return nil }
    let value = text[range].replacingOccurrences(of: #"[^0-9.]"#, with: "", options: .regularExpression)
    return Double(value)
}

@MainActor final class ControlModel: ObservableObject {
    @Published var showSharingShortcut = false
    let fileBridge = FileBridge()
    let sharingShortcut: SharingShortcutController
    @Published var panel = 0
    @Published var setupRequested = false
    @Published var invitationToReview: String?
    @Published private(set) var dismissedInvitations: [String: String]
    @Published var fingerprint = ""
    @Published var port = 4242
    @Published var authorized: [String: String] = [:]
    @Published var pendingFingerprints = Set<String>()
    @Published var pairing = false
    @Published var pairingNote: String?
    @Published var transport = "Checking route…"
    let discovery = Discovery()
    private var pairRequest: PairRequest?
    private var revoking: String?
    @Published var peers: [Peer] = []
    @Published var selected: Int?
    @Published var requestedControlsPeer: Int?
    @Published var connected = false
    @Published var running: Bool?
    @Published var busy = false
    @Published var requestedSharing: Bool?
    @Published var activity: String?
    var sharingEnabled: Bool { requestedSharing ?? (running == true) }
    var hasRemoteConnection: Bool { connected && peers.contains(where: { $0.alive }) }
    var homeTitle: String {
        if running == nil { return "Welcome to Lan Mouse" }
        if !sharingEnabled { return "Your desk, on pause" }
        if !connected { return "Connecting to sharing…" }
        if hasRemoteConnection { return "Your Macs are connected" }
        return peers.isEmpty ? "One mouse. Both Macs." : "Ready when your other Mac is"
    }
    var homeMessage: String {
        if running == nil { return "Checking this Mac’s sharing settings…" }
        if !sharingEnabled { return "Turn sharing on when you want to use one mouse and keyboard across your Macs." }
        if !connected { return "Waiting for the sharing service on this Mac. If this continues, open Troubleshooting." }
        if hasRemoteConnection { return "Move past the screen edge toward your other Mac to switch. Move back to return." }
        return peers.isEmpty ? "Connect your other Mac to move between computers without switching keyboards." : "Keep Lan Mouse open and sharing on at both computers, then move across the edge to connect."
    }
    @Published var capture = "Unknown"
    @Published var emulation = "Unknown"
    @Published var error: String?
    @Published var samples: [Sample] = []
    @Published var sampledTarget: String?
    @Published var traces: [TraceLine] = []
    @Published var logs = "Waiting for logs…"
    @Published var debugEvents: [String] = []
    let machine = Host.current().localizedName ?? "This Mac"
    private var link: IPCLink!
    private var pendingMapping: (Int, [String: Int])?
    private var pendingPosition: (Int, String)?
    private var didStart = false
    private let preferences: UserDefaults
    init(preferences: UserDefaults = .standard) {
        self.preferences = preferences
        sharingShortcut = SharingShortcutController(preferences: preferences)
        dismissedInvitations = preferences.dictionary(forKey: "dismissedInvitations") as? [String: String] ?? [:]
    }
    func canReviewInvitation(_ item: NearbyMac) -> Bool {
        guard let id = item.invitationID, UUID(uuidString: id) != nil else { return false }
        return item.invitationTarget == fingerprint && oppositePosition(item.invitationPosition ?? "") != nil && dismissedInvitations[item.fingerprint] != id
    }
    func dismissInvitation(_ item: NearbyMac) {
        guard canReviewInvitation(item), let id = item.invitationID else { return }
        dismissedInvitations[item.fingerprint] = id
        preferences.set(dismissedInvitations, forKey: "dismissedInvitations")
        if invitationToReview == id { invitationToReview = nil }
        discovery.clearProgress(invitation: id)
    }
    func rememberIdentity(_ fingerprint: String, for peer: Peer) {
        guard let fingerprint = normalizedFingerprint(fingerprint) else { return }
        var identities = preferences.dictionary(forKey: "pairedIdentities") as? [String: String] ?? [:]
        identities[peer.identityKey] = fingerprint
        preferences.set(identities, forKey: "pairedIdentities")
    }
    func verifiedIdentity(for peer: Peer) -> String? {
        guard let fp = (preferences.dictionary(forKey: "pairedIdentities") as? [String: String])?[peer.identityKey], authorized[fp] != nil else { return nil }
        return fp
    }

    var peer: Peer? { peers.first { $0.id == selected } }
    func displayName(for peer: Peer) -> String {
        discovery.nearby.first {
            $0.host.trimmingCharacters(in: CharacterSet(charactersIn: ".")) == peer.name.trimmingCharacters(in: CharacterSet(charactersIn: ".")) ||
            $0.ips.contains(peer.name) || $0.ips.contains(peer.target ?? "") || !Set($0.ips).isDisjoint(with: peer.ips)
        }?.name ?? peer.name
    }
    var successful: [Double] { samples.compactMap(\.ms) }
    var loss: Double { samples.isEmpty ? 0 : Double(samples.filter { $0.ms == nil }.count) / Double(samples.count) * 100 }
    var average: Double? { successful.isEmpty ? nil : successful.reduce(0, +) / Double(successful.count) }
    var p95: Double? { let sorted = successful.sorted(); return sorted.isEmpty ? nil : sorted[min(sorted.count - 1, Int(ceil(Double(sorted.count) * 0.95)) - 1)] }

    func start() {
        guard !didStart else { return }; didStart = true
        link = IPCLink(event: { [weak self] data in Task { @MainActor in self?.consume(data) } }, status: { [weak self] value in
            Task { @MainActor in
                guard let self else { return }
                self.connected = value
                self.fileBridge.connectionChanged()
                if value { self.running = true; self.publishSharingShortcut() }
                else {
                    self.capture = "Unknown"; self.emulation = "Unknown"
                    // Retain the desk while paused; remote liveness is no longer known.
                    for index in self.peers.indices { self.peers[index].alive = false; self.peers[index].pressed = false }
                    self.samples = []
                }
                self.trace(value ? "Connected to local daemon" : "Local daemon connection closed")
            }
        })
        sharingShortcut.onPress = { [weak self] in
            guard let self, self.running != nil, !self.busy else { return }
            self.setSharing(!self.sharingEnabled)
        }
        sharingShortcut.onChange = { [weak self] in self?.publishSharingShortcut() }
        sharingShortcut.start()
        fileBridge.start(self)
        link.start()
        Task { [weak self] in
            while let self {
                let status = await command("/bin/launchctl", ["print", self.service])
                let isRunning = status.code == 0 && status.text.contains("state = running")
                if !self.busy && self.running != isRunning { self.running = isRunning }
                if self.connected { self.send("Sync", record: false); self.publishSharingShortcut() }
                self.discovery.update(name: self.machine, fingerprint: self.fingerprint, port: self.port, enabled: self.connected)
                await self.probe()
                await self.readLogs()
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }
    var service: String { "gui/\(getuid())/de.feschber.lan-mouse" }
    func trace(_ text: String) { traces.append(TraceLine(text: text)); if traces.count > 150 { traces.removeFirst(traces.count - 150) } }
    func publishSharingShortcut() {
        guard connected else { return }
        send(["SetSharingShortcut": sharingShortcut.active.map { $0.ipc as Any } ?? NSNull()], record: false)
    }
    func consume(_ data: Data) {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        if let value = object["CaptureEntered"] as? [String: Any], let handle = value["handle"] as? Int { fileBridge.captured(handle: handle) }
        if let shortcut = object["SharingShortcutPressed"] as? [String: Any] { sharingShortcut.captured(shortcut) }
        if let rows = object["Enumerate"] as? [Any] {
            let updated = rows.compactMap(Peer.init)
            if peers != updated { peers = updated }
            if !peers.contains(where: { $0.id == selected }) { selected = peers.first?.id }
        }
        for key in ["Created", "State"] {
            if let value = object[key], let p = Peer(value) {
                if key == "Created", let request = pairRequest {
                    pairRequest = nil
                    send(["UpdateHostname": [p.id, request.host]])
                    send(["UpdatePort": [p.id, request.port]])
                    send(["UpdatePosition": [p.id, request.position]])
                    send(["Activate": [p.id, true]])
                    var configured = p; configured.name = request.host; configured.port = request.port
                    rememberIdentity(request.fingerprint, for: configured)
                    selected = p.id; pairing = false
                    pairingNote = "Configured. Repeat on the other Mac, then move across the chosen edge."
                }
                if let index = peers.firstIndex(where: { $0.id == p.id }) {
                    if peers[index] != p { peers[index] = p }
                } else { peers.append(p) }
                if selected == nil { selected = p.id }
                if let pending = pendingMapping, pending.0 == p.id, pending.1 == p.mappings {
                    trace("Mappings applied for \(p.name)"); pendingMapping = nil
                }
                if let pending = pendingPosition, pending.0 == p.id, pending.1 == p.position {
                    trace("Position applied: \(p.name) → \(p.position)"); pendingPosition = nil
                }
            }
        }
        if let fp = object["PublicKeyFingerprint"] as? String, fingerprint != fp { fingerprint = fp }
        if let value = object["PortChanged"] as? [Any], let p = value.first as? Int, port != p { port = p }
        if let value = object["AuthorizedUpdated"] as? [String: String] {
            if authorized != value { authorized = value }
            if !pendingFingerprints.isDisjoint(with: value.keys) { pendingFingerprints.subtract(value.keys) }
            if let fp = revoking, value[fp] == nil {
                revoking = nil
                let stored = try? String(contentsOfFile: NSHomeDirectory() + "/.config/lan-mouse/config.toml", encoding: .utf8)
                if let stored, !stored.contains(fp) { restart() }
                else {
                    setSharing(false)
                    error = "Trust removal could not be verified on disk. Sharing is stopped; fix the config file before turning it back on."
                }
            }
        }
        if let value = object["ConnectionAttempt"] as? [String: String], let fp = value["fingerprint"], authorized[fp] == nil {
            pendingFingerprints.insert(fp)
        }
        if let id = object["Deleted"] as? Int {
            if let peer = peers.first(where: { $0.id == id }) {
                var identities = preferences.dictionary(forKey: "pairedIdentities") as? [String: String] ?? [:]
                identities.removeValue(forKey: peer.identityKey)
                preferences.set(identities, forKey: "pairedIdentities")
            }
            peers.removeAll { $0.id == id }
        }
        if let value = object["CaptureStatus"] as? String, capture != value { capture = value }
        if let value = object["EmulationStatus"] as? String, emulation != value { emulation = value }
        if let value = object["Error"] as? String { error = value; trace("Error: \(value)") }
        if object["NoSuchClient"] != nil { error = "That computer no longer exists. Refresh and try again." }
        for key in ["DeviceConnected", "DeviceEntered", "IncomingDisconnected", "ConnectionAttempt"] where object[key] != nil {
            trace(String(decoding: data, as: UTF8.self))
        }
    }
    func send(_ value: Any, record: Bool = true) {
        guard let link else { error = "Sharing is not connected. Turn it on and try again."; return }
        if record { trace("Requested: \(value)") }
        link.send(value) { [weak self] success in
            if !success { Task { @MainActor in self?.error = "The connection did not respond. Reconnect and try again." } }
        }
    }
    func position(_ id: Int, _ position: String) {
        pendingPosition = (id, position); send(["UpdatePosition": [id, position]])
    }
    func mappings(_ id: Int, _ mappings: [String: Int]) {
        pendingMapping = (id, mappings); send(["UpdateKeyMap": [id, mappings]])
    }
    func setSharing(_ enabled: Bool) {
        guard !busy else { return }; busy = true; error = nil
        requestedSharing = enabled; activity = enabled ? "Starting sharing…" : "Stopping sharing…"
        Task {
            let domain = "gui/\(getuid())"
            let result: CommandResult
            if enabled {
                let enable = await command("/bin/launchctl", ["enable", service])
                let check = await command("/bin/launchctl", ["print", service])
                if enable.code != 0 { result = enable }
                else if check.code == 0 { result = await command("/bin/launchctl", ["kickstart", "-k", service]) }
                else { result = await command("/bin/launchctl", ["bootstrap", domain, NSHomeDirectory() + "/Library/LaunchAgents/de.feschber.lan-mouse.plist"]) }
            } else {
                let disable = await command("/bin/launchctl", ["disable", service])
                if disable.code == 0 { result = await command("/bin/launchctl", ["bootout", service]) }
                else { result = disable }
            }
            if result.code != 0 { error = result.text; trace("Service control failed: \(result.text)") }
            else { trace(enabled ? "Sharing started" : "Sharing stopped"); if !enabled { running = false } }
            let status = await command("/bin/launchctl", ["print", service])
            running = status.code == 0 && status.text.contains("state = running")
            requestedSharing = nil; activity = nil; busy = false
        }
    }
    func restart() {
        guard !busy else { return }; busy = true; activity = "Restarting sharing…"
        Task {
            let result = await command("/bin/launchctl", ["kickstart", "-k", service])
            if result.code != 0 { error = result.text }
            trace("Restart requested\(result.code == 0 ? "" : ": failed")"); activity = nil; busy = false
        }
    }
    func probe() async {
        guard connected, let peer, let target = peer.target, !target.hasPrefix("-") else { return }
        if sampledTarget != target {
            sampledTarget = target; samples = []
            let route = await command("/sbin/route", ["-n", "get", target])
            let iface = route.text.split(separator: "\n").first { $0.contains("interface:") }?.split(separator: ":").last?.trimmingCharacters(in: .whitespaces) ?? "unknown"
            let ports = await command("/usr/sbin/networksetup", ["-listallhardwareports"])
            let block = ports.text.components(separatedBy: "\n\n").first { $0.contains("Device: \(iface)\n") }
            let name = block?.components(separatedBy: "\n").first { $0.hasPrefix("Hardware Port:") }?.replacingOccurrences(of: "Hardware Port: ", with: "") ?? "Network"
            transport = "\(name) · \(iface)"
        }
        let result = await command(target.contains(":") ? "/sbin/ping6" : "/sbin/ping", ["-n", "-c", "1", target], timeout: 1.5)
        guard connected, self.peer?.target == target else { return }
        samples.append(Sample(ms: result.code == 0 ? pingTime(result.text) : nil))
        if samples.count > 120 { samples.removeFirst(samples.count - 120) }
    }
    func readLogs() async {
        let updated = await Task.detached(priority: .utility) {
            let url = URL(fileURLWithPath: "/tmp/lan-mouse-daemon.log")
            guard let file = try? FileHandle(forReadingFrom: url) else { return "No daemon log found." }
            defer { try? file.close() }
            guard let size = try? file.seekToEnd() else { return "Cannot read daemon log." }
            try? file.seek(toOffset: size > 32000 ? size - 32000 : 0)
            let data = (try? file.read(upToCount: 32000)) ?? Data()
            return String(decoding: data, as: UTF8.self).split(separator: "\n").suffix(100).joined(separator: "\n")
        }.value
        if logs != updated { logs = updated }
    }
    func recordInput(_ text: String) {
        debugEvents.insert(text, at: 0); if debugEvents.count > 30 { debugEvents.removeLast() }
    }
    func exportReport() {
        let panel = NSSavePanel(); panel.nameFieldStringValue = "lan-mouse-diagnostics.txt"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let report = "Lan Mouse diagnostics — \(Date())\nMachine: \(machine)\nCapture: \(capture), emulation: \(emulation)\nTarget: \(sampledTarget ?? "none")\nNetwork RTT avg: \(average.map(String.init(describing:)) ?? "unknown") ms\nProbe loss: \(loss)%\n\n" + traces.map { "\($0.date) \($0.text)" }.joined(separator: "\n") + "\n\nRecent daemon log:\n" + logs
        do { try report.write(to: url, atomically: true, encoding: .utf8) } catch { self.error = error.localizedDescription }
    }
}

extension ControlModel {
    func pair(_ request: PairRequest) {
        guard !pairing, connected else { return }
        guard let fp = normalizedFingerprint(request.fingerprint), fp != fingerprint,
              !request.host.hasPrefix("-"), !request.host.contains(where: { $0.isWhitespace }),
              !request.host.isEmpty, request.host.count < 254, (1...65535).contains(request.port), oppositePosition(request.position) != nil else { error = "Use the other Mac’s valid hostname/IP and fingerprint."; return }
        let normalize = { (s: String) in s.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: ".")) }
        let found = discovery.nearby.first { $0.fingerprint == fp }
        let existing = peers.first { normalize($0.name) == normalize(request.host) || $0.ips.contains(request.host) || !(Set($0.ips).intersection(found?.ips ?? []).isEmpty) }
        error = nil
        send(["AuthorizeKey": [found?.name ?? request.host, fp]])
        if let existing {
            send(["UpdateHostname": [existing.id, request.host]])
            send(["UpdatePort": [existing.id, request.port]])
            position(existing.id, request.position)
            send(["Activate": [existing.id, true]])
            var configured = existing; configured.name = request.host; configured.port = request.port
            rememberIdentity(fp, for: configured)
            selected = existing.id
            pairingNote = "Existing computer updated. Repeat on the other Mac, then cross the edge."
            return
        }
        pairing = true; pairRequest = request; pairingNote = nil
        send("Create")
        Task {
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            if pairing { pairing = false; pairRequest = nil; error = "No configuration response. Check the service and retry." }
        }
    }
    func disconnectPeer(_ peer: Peer, forget: Bool, revoke: String) {
        if forget { discovery.cancelInvitation(); send(["Delete": peer.id]) } else { send(["Activate": [peer.id, false]]) }
        if !revoke.isEmpty { self.revoke(revoke) }
    }
    func revoke(_ fp: String) {
        guard connected, !busy, authorized[fp] != nil else { return }
        revoking = fp; send(["RemoveAuthorizedKey": fp])
    }
    func installService() {
        guard !busy else { return }; busy = true
        do {
            let home = FileManager.default.homeDirectoryForCurrentUser
            let binary = home.appendingPathComponent(".local/bin/lan-mouse")
            let plist = home.appendingPathComponent("Library/LaunchAgents/de.feschber.lan-mouse.plist")
            let config = home.appendingPathComponent(".config/lan-mouse/config.toml")
            for file in [binary, plist, config] { try FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true) }
            if !FileManager.default.fileExists(atPath: binary.path) {
                guard let source = Bundle.main.url(forResource: "lan-mouse-daemon", withExtension: nil) else { throw CocoaError(.fileNoSuchFile) }
                try FileManager.default.copyItem(at: source, to: binary)
                try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: binary.path)
            }
            if !FileManager.default.fileExists(atPath: config.path) { try "".write(to: config, atomically: true, encoding: .utf8) }
            if !FileManager.default.fileExists(atPath: plist.path) {
                let spec: [String: Any] = ["Label": "de.feschber.lan-mouse", "ProgramArguments": [binary.path, "daemon"],
                    "RunAtLoad": true, "KeepAlive": true, "ThrottleInterval": 5, "ProcessType": "Interactive",
                    "StandardOutPath": "/tmp/lan-mouse-daemon.log", "StandardErrorPath": "/tmp/lan-mouse-daemon.log"]
                let data = try PropertyListSerialization.data(fromPropertyList: spec, format: .xml, options: 0)
                try data.write(to: plist, options: .atomic)
            }
            busy = false; setSharing(true)
        } catch { self.error = "Service installation failed: \(error.localizedDescription)"; busy = false }
    }
}
