import AppKit
import Carbon
import SwiftUI

struct SharingShortcut: Codable, Equatable {
    let keyCode: UInt32
    let modifiers: UInt32 // NSEvent's device-independent modifier bits.
    static let allowedModifiers: UInt32 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20)
    var valid: Bool {
        keyCode < 128 && ![54,55,56,57,58,59,60,61,62,63].contains(keyCode) &&
        modifiers & ~Self.allowedModifiers == 0 && modifiers & ((1 << 18) | (1 << 19) | (1 << 20)) != 0 &&
        modifiers != Self.allowedModifiers // Lan Mouse's emergency release chord.
    }
    var title: String { mouseModifierName(UInt(modifiers)) + mouseKeyName(Int(keyCode)) }
    var carbonModifiers: UInt32 {
        var value: UInt32 = 0
        for (native, carbon) in [(17, shiftKey), (18, controlKey), (19, optionKey), (20, cmdKey)] where modifiers & (1 << native) != 0 { value |= UInt32(carbon) }
        return value
    }
    var ipc: [String: Any] { ["key_code": keyCode, "modifiers": modifiers] }
}

@MainActor final class SharingShortcutController: ObservableObject {
    @Published private(set) var shortcut: SharingShortcut?
    @Published private(set) var error: String?
    private(set) var suspended = false
    private let preferences: UserDefaults
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private var held = false
    private var lastPress = Date.distantPast
    var onPress: (() -> Void)?
    var onChange: (() -> Void)?
    init(preferences: UserDefaults = .standard) {
        self.preferences = preferences
        if let data = preferences.data(forKey: "sharingShortcut"), let saved = try? JSONDecoder().decode(SharingShortcut.self, from: data), saved.valid { shortcut = saved }
    }
    var active: SharingShortcut? { suspended || hotKey == nil ? nil : shortcut }
    func start() {
        guard handler == nil else { return }
        var types = [EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed)),
                     EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyReleased))]
        let status = InstallEventHandler(GetApplicationEventTarget(), { _, event, context in
            guard let event, let context else { return OSStatus(eventNotHandledErr) }
            let controller = Unmanaged<SharingShortcutController>.fromOpaque(context).takeUnretainedValue()
            MainActor.assumeIsolated {
                if GetEventKind(event) == UInt32(kEventHotKeyReleased) { controller.held = false }
                else if !controller.held { controller.held = true; controller.trigger() }
            }
            return noErr
        }, types.count, &types, Unmanaged.passUnretained(self).toOpaque(), &handler)
        guard status == noErr else { error = "macOS couldn’t start keyboard shortcuts (\(status))."; return }
        resume()
    }
    func trigger() {
        guard !suspended, shortcut != nil, Date().timeIntervalSince(lastPress) > 0.8 else { return }
        lastPress = Date(); onPress?()
    }
    func captured(_ value: [String: Any]) {
        guard let active, value["key_code"] as? UInt32 == active.keyCode,
              value["modifiers"] as? UInt32 == active.modifiers else { return }
        trigger()
    }
    @discardableResult func save(_ candidate: SharingShortcut?) -> Bool {
        guard candidate?.valid != false else { error = "Include Command, Control, or Option and a key. The four-modifier emergency release is reserved."; return false }
        var replacement: EventHotKeyRef?
        if let candidate {
            if candidate == shortcut && hotKey != nil && !suspended { return true }
            let status = RegisterEventHotKey(candidate.keyCode, candidate.carbonModifiers,
                                            EventHotKeyID(signature: 0x4C4D5348, id: 1), GetApplicationEventTarget(), UInt32(kEventHotKeyExclusive), &replacement)
            guard status == noErr else { error = "This shortcut is unavailable or used by another app. Choose a different combination. (\(status))"; return false }
        }
        if let hotKey { UnregisterEventHotKey(hotKey) }
        hotKey = replacement; shortcut = candidate; suspended = false; held = false; error = nil
        if let candidate, let data = try? JSONEncoder().encode(candidate) { preferences.set(data, forKey: "sharingShortcut") }
        else { preferences.removeObject(forKey: "sharingShortcut") }
        onChange?(); return true
    }
    func suspend() {
        suspended = true; held = false
        if let hotKey { UnregisterEventHotKey(hotKey) }; hotKey = nil; onChange?()
    }
    func resume() { suspended = false; _ = save(shortcut) }
    deinit {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        if let handler { RemoveEventHandler(handler) }
    }
}

struct SharingShortcutSettings: View {
    @ObservedObject var shortcuts: SharingShortcutController
    @Environment(\.dismiss) private var dismiss
    @State private var recording = false
    @State private var candidate: SharingShortcut?
    @State private var flags: UInt = 0
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Sharing Shortcut").font(.headline)
            Text("Turn sharing on or off from any app, even while controlling your other Mac.")
                .foregroundStyle(.secondary)
            HStack {
                Text("Toggle sharing")
                Spacer()
                Button(recording ? (flags == 0 ? "Press shortcut…" : mouseModifierName(flags)) : candidate?.title ?? "Record Shortcut…") {
                    flags = 0; recording = true
                }.frame(minWidth: 150)
            }
            if recording {
                Text("Press a key with Command, Control, or Option. Escape cancels recording.").font(.caption).foregroundStyle(.secondary)
                MouseKeyRecorder(modifiersOnly: false, flagsChanged: { flags = $0 }, finish: { key, flags in
                    if let key { candidate = SharingShortcut(keyCode: UInt32(key), modifiers: UInt32(flags)) }
                    recording = false
                }, cancel: { recording = false }).frame(width: 1, height: 1)
            }
            if let candidate, !candidate.valid {
                Text("Include Command, Control, or Option. The four-modifier emergency release is reserved.").font(.caption).foregroundStyle(.orange)
            }
            if let error = shortcuts.error { Text(error).font(.caption).foregroundStyle(.orange) }
            Text("Keep Lan Mouse Control running; closing its window is fine. Set a shortcut on each Mac you use a keyboard with.")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("Remove Shortcut") { candidate = nil; recording = false }.disabled(candidate == nil)
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Save") { if shortcuts.save(candidate) { dismiss() } }
                    .keyboardShortcut(.defaultAction).disabled(recording || candidate?.valid == false)
            }
        }.padding(24).frame(width: 420)
            .onAppear { candidate = shortcuts.shortcut; shortcuts.suspend() }
            .onDisappear { shortcuts.resume() }
    }
}

// First responder only inside the recording sheet; no global event tap.
struct MouseKeyRecorder: NSViewRepresentable {
    let modifiersOnly: Bool
    let flagsChanged: (UInt) -> Void
    let finish: (Int?, UInt) -> Void
    let cancel: () -> Void
    func makeNSView(context: Context) -> Recorder { let view = Recorder(); view.owner = self; return view }
    func updateNSView(_ view: Recorder, context: Context) { view.owner = self }
    final class Recorder: NSView {
        var owner: MouseKeyRecorder!
        var lastFlags: UInt = 0
        var finished = false
        override var acceptsFirstResponder: Bool { true }
        override func viewDidMoveToWindow() { DispatchQueue.main.async { [weak self] in if let self { self.window?.makeFirstResponder(self) } } }
        private func modifiers(_ event: NSEvent) -> UInt { event.modifierFlags.intersection([.control, .option, .shift, .command]).rawValue }
        override func flagsChanged(with event: NSEvent) {
            guard !finished else { return }
            let flags = modifiers(event)
            if owner.modifiersOnly && flags == 0 && lastFlags != 0 { finished = true; owner.finish(nil, lastFlags) }
            else { lastFlags |= flags; owner.flagsChanged(flags) }
        }
        override func keyDown(with event: NSEvent) {
            guard !finished else { return }
            if event.keyCode == 53 { finished = true; owner.cancel() }
            else if !owner.modifiersOnly { finished = true; owner.finish(Int(event.keyCode), modifiers(event)) }
        }
        override func performKeyEquivalent(with event: NSEvent) -> Bool {
            guard window?.firstResponder === self else { return false }
            keyDown(with: event); return true
        }
    }
}
