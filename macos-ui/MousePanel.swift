import AppKit
import SwiftUI

struct MousePanel: View {
    @ObservedObject var model: ControlModel
    @StateObject private var mouse = MouseProfileModel()
    @State private var tab = "Buttons"
    @State private var adding = false
    @State private var options = false
    @State private var restoring = false
    @State private var recording: MouseRecording?
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                PageHeading(title: "Mouse", subtitle: "Mac Mouse Fix controls, built into Lan Mouse.")
                if !mouse.installed {
                    ContentUnavailableView("Mouse engine is missing", systemImage: "computermouse", description: Text("Reinstall the complete Lan Mouse app to restore its bundled mouse engine."))
                } else if !mouse.compatible {
                    Text("Your mouse preferences couldn’t be loaded. Your existing settings are safe.").foregroundStyle(.secondary)
                    Button("Restart Mouse Engine") { mouse.restartEngine() }
                } else {
                    if !mouse.helperRunning || mouse.accessibilityGranted != true { engineHelp }
                    Picker("Mouse settings", selection: $tab) {
                        Text("Buttons").tag("Buttons"); Text("Scrolling").tag("Scrolling")
                    }.pickerStyle(.segmented).labelsHidden().frame(width: 260).frame(maxWidth: .infinity)
                    Group { if tab == "Buttons" { buttons } else { scrolling } }.disabled(mouse.busy)
                    Divider()
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Toggle("Use the same settings on both Macs", isOn: Binding(get: { mouse.syncEnabled }, set: mouse.setSync))
                                .toggleStyle(.checkbox).disabled(mouse.busy)
                            Spacer()
                            if mouse.busy { ProgressView().controlSize(.small).accessibilityLabel("Saving mouse settings") }
                        }
                        Text(mouse.syncEnabled ? "Changes save automatically and sync while connected." : "Changes save automatically on this Mac.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    HStack {
                        Text("Powered by Mac Mouse Fix").foregroundStyle(.tertiary)
                        Spacer()
                        Button("Test Mouse & Keyboard") { model.panel = 3 }.buttonStyle(.link)
                    }.font(.caption)
                }
                if let error = mouse.error { Label(error, systemImage: "exclamationmark.triangle").font(.callout).foregroundStyle(.orange).textSelection(.enabled) }
            }.frame(maxWidth: 680).frame(maxWidth: .infinity, alignment: .leading)
        }.task { mouse.start() }
            .sheet(isPresented: $adding) { AddMouseAction(mouse: mouse) }
            .sheet(isPresented: $restoring) { restoreSheet }
            .sheet(item: $recording) { MouseRecordingSheet(request: $0) }
    }
    private var buttons: some View {
        VStack(alignment: .leading, spacing: 12) {
            Toggle("Enable button actions", isOn: enabled("buttonKillSwitch"))
            VStack(alignment: .leading, spacing: 0) {
                if mouse.remaps.isEmpty {
                    Button { adding = true } label: {
                        VStack(spacing: 10) {
                            Image(systemName: "plus").font(.system(size: 28, weight: .light))
                            Text("Add a button action")
                        }.frame(maxWidth: .infinity).padding(28)
                    }.buttonStyle(.plain)
                } else {
                    ForEach(Array(Set(mouse.remaps.map(\.button))).sorted(), id: \.self) { button in
                        Text(mouseButtonName(button)).font(.system(size: 12, weight: .semibold)).foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 12).padding(.vertical, 5)
                            .background(Color(nsColor: .quaternaryLabelColor).opacity(0.35))
                        ForEach(Array(mouse.remaps.enumerated()).filter { $0.element.button == button }, id: \.offset) { _, row in
                            Divider()
                            HStack(spacing: 10) {
                                Button { mouse.removeAction(row) } label: { Image(systemName: "minus.circle.fill").foregroundStyle(.secondary) }
                                    .buttonStyle(.borderless).help("Remove \(mouseButtonName(button)) — \(row.title)")
                                    .accessibilityLabel("Remove \(mouseButtonName(button)) \(row.title)")
                                Text(row.title).frame(maxWidth: .infinity, alignment: .leading).fixedSize(horizontal: false, vertical: true)
                                actionPicker(row).frame(maxWidth: .infinity)
                            }.padding(.horizontal, 10).padding(.vertical, 6)
                        }
                    }
                }
            }.background(Color(nsColor: .controlBackgroundColor)).clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color(nsColor: .separatorColor)))
            HStack {
                Button { adding = true } label: { Label("Add Action…", systemImage: "plus") }
                Button("Options…") { options.toggle() }.popover(isPresented: $options) {
                    VStack(alignment: .leading, spacing: 8) {
                        Toggle("Keep the pointer still while dragging", isOn: Binding(get: { mouse.general["lockPointerDuringDrag"] as? Bool ?? false }, set: { mouse.set("General", "lockPointerDuringDrag", $0) }))
                        Text("Applies to Click and Drag gestures.").font(.caption).foregroundStyle(.secondary)
                    }.padding(18).disabled(mouse.busy)
                }
                Spacer()
                Button("Restore Defaults…") { restoring = true }
            }.controlSize(.small)
            if mouse.remaps.contains(where: { $0.kind == "dragTrigger" && $0.actionID == "pan" }) {
                Text("Scroll & Navigate pans like a two-finger trackpad gesture. Hold the assigned button and move your mouse to scroll or navigate.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if mouse.remaps.contains(where: { $0.kind == "dragTrigger" && $0.actionID == "zoom" }) {
                Text("Zoom In or Out: hold the assigned button and drag up to zoom in, down to zoom out. Release to stop.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if mouse.general["buttonKillSwitch"] as? Bool == true {
                Text("Button actions are paused. Enable them above to use these assignments.").font(.caption).foregroundStyle(.secondary)
            }
        }
    }
    private func actionPicker(_ row: MouseRemap) -> some View {
        Picker("\(mouseButtonName(row.button)) — \(row.title)", selection: Binding(get: { row.actionID }, set: { id in
            if id == "record" {
                recording = MouseRecording(title: "Keyboard Shortcut", modifiersOnly: false) { key, flags in
                    if let key { mouse.changeAction(row, effect: ["type": "keyboardShortcut", "keycode": key, "flags": flags]) }
                }
            } else if let action = row.actions.first(where: { $0.id == id }) { mouse.changeAction(row, effect: action.effect) }
        })) {
            ForEach(row.actions.filter { !$0.id.hasPrefix("media:") }) { Text($0.title).tag($0.id) }
            if row.kind == "click" {
                Divider(); Text("Keyboard Shortcut…").tag("record"); Divider()
                ForEach(row.actions.filter { $0.id.hasPrefix("media:") }) { Text($0.title).tag($0.id) }
            }
            if row.actionID == "custom" { Text(row.actionTitle).tag("custom") }
        }.labelsHidden().help(row.kind == "dragTrigger" && row.actionID == "pan" ? "Pan and scroll like a two-finger trackpad gesture" : row.actionTitle)
    }
    private var scrolling: some View {
        VStack(alignment: .leading, spacing: 20) {
            Toggle("Enable scrolling adjustments", isOn: enabled("scrollKillSwitch"))
            Grid(alignment: .leading, horizontalSpacing: 20, verticalSpacing: 16) {
                GridRow(alignment: .top) {
                    Text("Smoothness:").gridColumnAlignment(.trailing).padding(.top, 3)
                    VStack(alignment: .leading, spacing: 8) {
                        Picker("Smoothness", selection: scrollString("smooth", fallback: "off")) {
                            Text("Off").tag("off"); Text("Regular").tag("regular"); Text("High").tag("high")
                            if mouse.scroll["smooth"] as? String == "low" { Text("Low (legacy)").tag("low") }
                        }.labelsHidden().frame(width: 180)
                        if mouse.scroll["smooth"] as? String == "high" {
                            Toggle("Trackpad Simulation", isOn: scrollBool("trackpadSimulation"))
                            Text("Move between pages in Safari by scrolling horizontally, and more.").font(.caption).foregroundStyle(.secondary)
                        }
                        Toggle("Reverse Direction", isOn: scrollBool("reverseDirection"))
                    }
                }
                GridRow(alignment: .top) {
                    Text("Speed:").padding(.top, 3)
                    VStack(alignment: .leading, spacing: 8) {
                        Picker("Speed", selection: scrollString("speed", fallback: "system")) {
                            Text("macOS").tag("system"); Text("Low").tag("low"); Text("Medium").tag("medium"); Text("High").tag("high")
                        }.labelsHidden().frame(width: 180)
                        if mouse.scroll["speed"] as? String != "system" {
                            Toggle("Precision", isOn: scrollBool("precise"))
                            Text("Scroll precisely by moving the scroll wheel slowly.").font(.caption).foregroundStyle(.secondary)
                        } else { Text("Use the scroll speed set in macOS.").font(.caption).foregroundStyle(.secondary) }
                    }
                }
                GridRow(alignment: .top) {
                    Text("Keyboard Modifiers:").padding(.top, 4)
                    VStack(alignment: .leading, spacing: 10) {
                        ForEach(["horizontal", "zoom", "swift", "precise"], id: \.self) { key in
                            HStack(spacing: 12) {
                                let flags = (mouse.scroll["modifiers"] as? [String: Any])?[key] as? UInt ?? 0
                                Button(flags == 0 ? "None" : mouseModifierName(flags)) {
                                    recording = MouseRecording(title: modifierTitle(key), modifiersOnly: true) { _, flags in
                                        mouse.edit { setMouseScrollModifier(in: &$0, key: key, flags: flags) }
                                    }
                                }.frame(width: 72).accessibilityLabel("\(modifierTitle(key)): \(flags == 0 ? "None" : mouseModifierName(flags))")
                                Text(modifierTitle(key))
                            }
                        }
                        Button("Restore Defaults") { mouse.set("Scroll", "modifiers", defaultMouseScrollModifiers) }.controlSize(.small)
                            .disabled(NSDictionary(dictionary: mouse.scroll["modifiers"] as? [String: Any] ?? [:]).isEqual(to: defaultMouseScrollModifiers))
                        Text("Hold a modifier while scrolling. Click a key to change it.").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }.frame(maxWidth: .infinity, alignment: .center).disabled(mouse.general["scrollKillSwitch"] as? Bool ?? false)
        }
    }
    private func modifierTitle(_ key: String) -> String {
        ["horizontal": "Scroll Horizontally", "zoom": "Zoom In or Out", "swift": "Scroll Swiftly", "precise": "Scroll Precisely"][key] ?? key
    }
    private func enabled(_ key: String) -> Binding<Bool> { Binding(get: { !(mouse.general[key] as? Bool ?? false) }, set: { mouse.set("General", key, !$0) }) }
    private func scrollBool(_ key: String) -> Binding<Bool> { Binding(get: { mouse.scroll[key] as? Bool ?? false }, set: { mouse.set("Scroll", key, $0) }) }
    private func scrollString(_ key: String, fallback: String) -> Binding<String> { Binding(get: { mouse.scroll[key] as? String ?? fallback }, set: { mouse.set("Scroll", key, $0) }) }
    private var restoreSheet: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Restore button defaults?").font(.headline)
            Text("Replace your button actions with Mac Mouse Fix’s defaults. Scrolling settings stay as they are. With sync enabled, these actions also replace those on the other Mac.")
            HStack {
                Button("Cancel") { restoring = false }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("3-Button Mouse") { mouse.restoreButtons("threeButtons"); restoring = false }
                Button("5-Button Mouse") { mouse.restoreButtons("fiveButtons"); restoring = false }
            }
        }.padding(24).frame(width: 450)
    }
    private var engineHelp: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 8) {
                Label(mouse.helperRunning ? "Allow mouse control" : "Mouse engine is starting", systemImage: "hand.raised")
                Text(mouse.helperRunning ? "In macOS Accessibility settings, allow Mac Mouse Fix Helper—the mouse engine inside Lan Mouse." : "The bundled engine runs in the background, even when this panel is closed.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("Accessibility Settings…") { NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!) }
                    Button("Show Engine in Finder") { NSWorkspace.shared.activateFileViewerSelecting([MouseProfileModel.helperURL]) }
                    Button("Retry") { mouse.restartEngine() }
                }.controlSize(.small)
            }.padding(6)
        }
    }
}
private struct AddMouseAction: View {
    @ObservedObject var mouse: MouseProfileModel
    @Environment(\.dismiss) private var dismiss
    @State private var button = 5
    @State private var gesture = "click"
    @State private var flags: UInt = 0
    @State private var action = "smartZoom"
    @State private var shortcut: [String: Any]?
    @State private var recording: MouseRecording?
    @State private var submitted = false
    private var actions: [MouseAction] { MouseAction.options(for: gesture) }
    private var alreadyAssigned: Bool {
        let candidate = MouseRemap.make(button: button, gesture: gesture, flags: flags, effect: [:])
        return mouse.remaps.contains { $0.hasSameTrigger(as: candidate) }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Add a Button Action").font(.headline)
            Form {
                Picker("Button", selection: $button) { ForEach(3...16, id: \.self) { Text(mouseButtonName($0)).tag($0) } }
                Picker("Gesture", selection: $gesture) {
                    Text("Click").tag("click"); Text("Double Click").tag("double"); Text("Click and Hold").tag("hold")
                    Text("Click and Drag").tag("dragTrigger"); Text("Click and Scroll").tag("scrollTrigger")
                }.onChange(of: gesture) { _, _ in action = actions[0].id; shortcut = nil }
                LabeledContent("Keyboard modifiers") {
                    Button(flags == 0 ? "None" : mouseModifierName(flags)) {
                        recording = MouseRecording(title: "Keyboard Modifiers", modifiersOnly: true) { _, value in flags = value }
                    }
                }
                Picker("Action", selection: $action) {
                    ForEach(actions) { Text($0.title).tag($0.id) }
                    if !["dragTrigger", "scrollTrigger"].contains(gesture) {
                        Text("Keyboard Shortcut…").tag("record")
                        if let shortcut { Text(mouseModifierName(shortcut["flags"] as? UInt ?? 0) + mouseKeyName(shortcut["keycode"] as? Int ?? 0)).tag("shortcut") }
                    }
                }.onChange(of: action) { old, new in
                    if new == "record" {
                        action = old
                        recording = MouseRecording(title: "Keyboard Shortcut", modifiersOnly: false) { key, flags in
                            if let key { shortcut = ["type": "keyboardShortcut", "keycode": key, "flags": flags]; action = "shortcut" }
                        }
                    }
                }
            }
            Text("Button numbers follow your mouse. The middle button is Button 3; side buttons are usually 4 and 5.").font(.caption).foregroundStyle(.secondary)
            if alreadyAssigned {
                Text("This gesture already has an action. Edit it in the table, or choose another gesture.").font(.caption).foregroundStyle(.secondary)
            }
            if submitted, let error = mouse.error { Text(error).font(.caption).foregroundStyle(.orange) }
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction).disabled(mouse.busy)
                Spacer()
                Button("Add Action") {
                    if let effect = action == "shortcut" ? shortcut : actions.first(where: { $0.id == action })?.effect {
                        submitted = true
                        mouse.addAction(.make(button: button, gesture: gesture, flags: flags, effect: effect))
                    }
                }.keyboardShortcut(.defaultAction).disabled(mouse.busy || alreadyAssigned)
            }
        }.padding(24).frame(width: 410).sheet(item: $recording) { MouseRecordingSheet(request: $0) }
            .onChange(of: mouse.busy) { _, busy in if submitted && !busy && mouse.error == nil { dismiss() } }
    }
}
private struct MouseRecording: Identifiable {
    let id = UUID()
    let title: String
    let modifiersOnly: Bool
    let save: (Int?, UInt) -> Void
}
private struct MouseRecordingSheet: View {
    let request: MouseRecording
    @Environment(\.dismiss) private var dismiss
    @State private var flags: UInt = 0
    var body: some View {
        VStack(spacing: 16) {
            Text(request.title).font(.headline)
            Text(request.modifiersOnly ? "Press and release Shift, Control, Option, or Command. You can combine keys." : "Press the keyboard shortcut you want this button to perform.")
                .multilineTextAlignment(.center).foregroundStyle(.secondary)
            Text(flags == 0 ? "Listening…" : mouseModifierName(flags)).font(.title2).frame(height: 38)
                .background(MouseKeyRecorder(modifiersOnly: request.modifiersOnly, flagsChanged: { flags = $0 }, finish: { key, flags in request.save(key, flags); dismiss() }, cancel: { dismiss() }).frame(width: 1, height: 1))
            HStack {
                Button("Cancel") { dismiss() }
                if request.modifiersOnly { Button("Clear Modifier") { request.save(nil, 0); dismiss() } }
            }
        }.padding(24).frame(width: 360)
    }
}
