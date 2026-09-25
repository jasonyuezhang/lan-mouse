// Companion to examples/native_file_drag_check.rs. Never sends to a real app.
import AppKit

private let root = URL(fileURLWithPath: ProcessInfo.processInfo.environment["LAN_MOUSE_NATIVE_DRAG_TEST_DIR"]!)
private let fixture = root.appendingPathComponent("fixture.txt")

@MainActor final class NativeDropTestView: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL])
    }
    required init?(coder: NSCoder) { fatalError() }
    override func draw(_ rect: NSRect) {
        NSColor.systemGreen.setFill(); bounds.fill()
        ("Lan Mouse drag test — closes automatically" as NSString).draw(at: NSPoint(x: 20, y: 90), withAttributes: [.font: NSFont.systemFont(ofSize: 16), .foregroundColor: NSColor.black])
    }
    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let urls = sender.draggingPasteboard.readObjects(forClasses: [NSURL.self]) as? [URL] ?? []
        guard urls == [fixture], (try? String(contentsOf: fixture, encoding: .utf8)) == "Lan Mouse native drag test\n" else { return false }
        try? Data("accepted".utf8).write(to: root.appendingPathComponent("result"), options: .atomic)
        return true
    }
}
@MainActor final class NativeDragTestDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var relay: FileDragRelay?
    private var timer: Timer?
    func applicationDidFinishLaunching(_ note: Notification) {
        if CommandLine.arguments.contains("receiver") {
            let timer = Timer(timeInterval: 0.01, repeats: true) { [weak self] _ in MainActor.assumeIsolated {
                let signal = root.appendingPathComponent("start")
                guard FileManager.default.fileExists(atPath: signal.path) else { return }
                try? FileManager.default.removeItem(at: signal)
                let relay = FileDragRelay(url: fixture, send: { request in
                    if request["BeginNativeFileDrag"] != nil {
                        try? JSONSerialization.data(withJSONObject: request).write(to: root.appendingPathComponent("request.json"), options: .atomic)
                    }
                }, started: {
                    try? Data("started".utf8).write(to: root.appendingPathComponent("started"), options: .atomic)
                }, finished: { copied in
                    try? Data((copied ? "copied" : "cancelled").utf8).write(to: root.appendingPathComponent("finished"), options: .atomic)
                    self?.relay = nil
                })
                self?.relay = relay; relay.begin()
            }}
            RunLoop.main.add(timer, forMode: .common); self.timer = timer
            try? Data().write(to: root.appendingPathComponent("receiver-ready"))
        } else {
            let window = NSWindow(contentRect: NSRect(x: 300, y: 300, width: 480, height: 200), styleMask: [.titled], backing: .buffered, defer: false)
            window.title = "Lan Mouse Native Drag Test"; window.isMovable = false; window.level = .floating
            window.contentView = NativeDropTestView(frame: NSRect(x: 0, y: 0, width: 480, height: 200))
            window.makeKeyAndOrderFront(nil); self.window = window
            NSApp.activate(ignoringOtherApps: true)
            let frame = window.frame
            let point = ["x": frame.midX, "y": (NSScreen.screens.first?.frame.maxY ?? 0) - frame.midY]
            try? JSONSerialization.data(withJSONObject: point).write(to: root.appendingPathComponent("target.json"), options: .atomic)
        }
    }
}
@main struct NativeDragTestApp {
    @MainActor static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(CommandLine.arguments.contains("receiver") ? .accessory : .regular)
        let delegate = NativeDragTestDelegate(); app.delegate = delegate; app.run()
    }
}
