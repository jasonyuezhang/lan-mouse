import SwiftUI
import Charts
import UniformTypeIdentifiers
import AppKit

// Explicit property-wrapper alias for SDKs that also expose a State macro.
typealias ViewState<Value> = SwiftUI.State<Value>

@main struct LanMouseControl: App {
    @StateObject private var model: ControlModel
    init() {
        let model = ControlModel()
        _model = StateObject(wrappedValue: model)
        // Menu-bar-only launches must receive files and shortcuts too.
        DispatchQueue.main.async { model.start() }
    }
    var body: some Scene {
        Window("Lan Mouse", id: "dashboard") {
            GeometryReader { geometry in
                Dashboard(model: model)
                    .frame(width: geometry.size.width, height: geometry.size.height)
            }.frame(minWidth: 780, minHeight: 560)
        }.defaultSize(width: 820, height: 620)
        MenuBarExtra("Lan Mouse", systemImage: model.running == true ? "computermouse.fill" : "computermouse") {
            MenuPanel(model: model)
        }.menuBarExtraStyle(.window)
    }
}

struct MenuPanel: View {
    @ObservedObject var model: ControlModel
    @Environment(\.openWindow) var openWindow
    @ViewState private var menuAnchor = NSView()
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 7) {
                Circle().fill(model.hasRemoteConnection ? Color.green : Color.secondary)
                    .frame(width: 7, height: 7)
                    .accessibilityLabel(model.hasRemoteConnection ? "Connected to another Mac" : "No remote connection")
                    .help(model.hasRemoteConnection ? "Connected to another Mac" : "No connection to another Mac")
                Toggle("Sharing", isOn: Binding(get: { model.sharingEnabled }, set: model.setSharing))
                    .toggleStyle(.switch).controlSize(.small)
                    .help("Share mouse and keyboard between your Macs")
                    .disabled(model.busy || model.running == nil)
            }
            .padding(.horizontal, 8).padding(.vertical, 6)
            if let activity = model.activity {
                Text(activity).font(.caption).foregroundStyle(.secondary)
            }
            if let error = model.error {
                Text(error).font(.caption).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
            }
            Divider()
            ForEach([(0, "Home"), (4, "Computers"), (1, "Arrangement"), (5, "Mouse"), (2, "Keys & Buttons"), (3, "Troubleshooting")], id: \.0) { index, name in
                MenuOption(title: name) {
                    menuAnchor.window?.orderOut(nil)
                    model.panel = index; openWindow(id: "dashboard"); NSApp.activate(ignoringOtherApps: true)
                }
            }
            Divider()
            SharingShortcutMenu(shortcuts: model.sharingShortcut) {
                menuAnchor.window?.orderOut(nil)
                openWindow(id: "dashboard"); NSApp.activate(ignoringOtherApps: true)
                model.showSharingShortcut = true
            }
            Divider()
            MenuOption(title: "Quit Control Panel") { NSApp.terminate(nil) }
            Text("Sharing continues when you quit.").font(.system(size: 11)).foregroundStyle(.secondary).padding(.horizontal, 8).padding(.bottom, 4)
        }.font(.system(size: 13)).padding(6).frame(width: 236)
            .background(MenuWindowAnchor(view: menuAnchor).frame(width: 0, height: 0))
            .onExitCommand { menuAnchor.window?.orderOut(nil) }
    }

}

private struct SharingShortcutMenu: View {
    @ObservedObject var shortcuts: SharingShortcutController
    let action: () -> Void
    var body: some View {
        MenuOption(title: "Sharing Shortcut…", detail: shortcuts.shortcut?.title ?? "Not set", action: action)
        if let error = shortcuts.error { Text(error).font(.caption).foregroundStyle(.orange) }
    }
}

private struct MenuOption: View {
    let title: String
    var detail: String? = nil
    let action: () -> Void
    @ViewState private var hovered = false
    var body: some View {
        Button(action: action) {
            HStack { Text(title); Spacer(); if let detail { Text(detail).foregroundStyle(.secondary).font(.caption) } }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 8).padding(.vertical, 4)
                .foregroundStyle(hovered ? Color(nsColor: .selectedMenuItemTextColor) : Color.primary)
                .background(hovered ? Color(nsColor: .selectedContentBackgroundColor) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
                .contentShape(Rectangle())
        }.buttonStyle(.plain)
            .accessibilityLabel(title)
            .onHover { hovered = $0 }
    }
}

// Close this popover's own window; the main window may already be key.
private struct MenuWindowAnchor: NSViewRepresentable {
    let view: NSView
    func makeNSView(context: Context) -> NSView { view }
    func updateNSView(_ nsView: NSView, context: Context) {}
}

struct PageHeading: View {
    let title: String
    let subtitle: String
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.title2.weight(.semibold))
            Text(subtitle).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }
}

// One button owns the whole header, including the title and empty row space.
// Native button semantics keep keyboard and VoiceOver interaction available.
struct WholeHeaderDisclosureStyle: DisclosureGroupStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { configuration.isExpanded.toggle() }
            } label: {
                HStack(spacing: 7) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .rotationEffect(.degrees(configuration.isExpanded ? 90 : 0))
                        .foregroundStyle(.secondary).accessibilityHidden(true)
                    configuration.label.frame(maxWidth: .infinity, alignment: .leading)
                }.padding(.vertical, 5).contentShape(Rectangle())
            }.buttonStyle(.plain)
                .accessibilityValue(configuration.isExpanded ? "Expanded" : "Collapsed")
            if configuration.isExpanded {
                configuration.content.padding(.leading, 17).padding(.top, 4)
            }
        }
    }
}

struct Dashboard: View {
    @ObservedObject var model: ControlModel
    private let pages = [(0, "Home", "house"), (4, "Computers", "desktopcomputer"),
                         (1, "Arrangement", "rectangle.3.group"), (5, "Mouse", "computermouse"), (2, "Keys & Buttons", "keyboard"),
                         (3, "Troubleshooting", "wrench.and.screwdriver")]
    var body: some View {
        HStack(spacing: 0) {
            List(selection: Binding<Int?>(get: { model.panel }, set: { if let value = $0 { model.panel = value } })) {
                ForEach(pages, id: \.0) { page in
                    Label(page.1, systemImage: page.2).tag(page.0).padding(.vertical, 3)
                }
            }.listStyle(.sidebar)
                .safeAreaInset(edge: .bottom) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("This Mac").font(.caption).foregroundStyle(.secondary)
                        Text(model.machine).font(.callout).lineLimit(2)
                    }.frame(maxWidth: .infinity, alignment: .leading).padding(14)
                }
                .frame(width: 172)
            Divider()
            VStack(spacing: 0) {
                HStack {
                    Text(pages.first { $0.0 == model.panel }?.1 ?? "Lan Mouse").font(.headline)
                    Spacer()
                    if let activity = model.activity { Text(activity).font(.caption).foregroundStyle(.secondary) }
                    Toggle("Sharing", isOn: Binding(get: { model.sharingEnabled }, set: model.setSharing))
                        .toggleStyle(.switch).controlSize(.small).disabled(model.busy || model.running == nil)
                        .help("Turn mouse and keyboard sharing on or off for this Mac")
                }.padding(.horizontal, 20).padding(.vertical, 12)
                Divider()
                InvitationBanner(model: model, discovery: model.discovery)
                if let error = model.error {
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Something needs attention").font(.callout.weight(.medium))
                            DisclosureGroup("Show details") { Text(error).font(.caption).textSelection(.enabled) }
                        }
                        Spacer()
                        Button("Dismiss") { model.error = nil }.controlSize(.small)
                    }.padding(12).background(.orange.opacity(0.08))
                }
                // Keep editors mounted so switching pages preserves unsaved edits and setup progress.
                ZStack {
                    ScrollView { home.padding(20) }.pageVisibility(model.panel == 0)
                    ScrollView { Arrangement(model: model).padding(20) }.pageVisibility(model.panel == 1)
                    MousePanel(model: model).padding(20).pageVisibility(model.panel == 5)
                    MappingPanel(model: model).padding(20).pageVisibility(model.panel == 2)
                    DebugPanel(model: model).padding(20).pageVisibility(model.panel == 3)
                    ScrollView { PairingPanel(model: model).padding(20) }.pageVisibility(model.panel == 4)
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }.background(Color(nsColor: .windowBackgroundColor))
        }.disclosureGroupStyle(WholeHeaderDisclosureStyle())
            .sheet(isPresented: $model.showSharingShortcut) { SharingShortcutSettings(shortcuts: model.sharingShortcut) }
    }
    var home: some View {
        VStack(alignment: .leading, spacing: 20) {
            PageHeading(title: model.homeTitle, subtitle: model.homeMessage)
            SharingShortcutMenu(shortcuts: model.sharingShortcut) { model.showSharingShortcut = true }
            FileBridgeSettings(bridge: model.fileBridge)
            GroupBox {
                VStack(spacing: 16) {
                    HStack(spacing: 20) {
                        computer(model.machine, local: true)
                        Image(systemName: model.hasRemoteConnection ? "arrow.left.arrow.right" : "ellipsis")
                            .font(.title3).foregroundStyle(model.hasRemoteConnection ? .green : .secondary)
                        computer((model.peers.first(where: { $0.alive }) ?? model.peer).map { model.displayName(for: $0) } ?? "Your other Mac", local: false)
                    }.frame(maxWidth: .infinity).padding(.vertical, 14)
                    Divider()
                    HStack {
                        Label(model.hasRemoteConnection ? "Connected" : model.running == false ? "Sharing is off" : "No remote connection",
                              systemImage: model.hasRemoteConnection ? "checkmark.circle.fill" : "circle")
                            .font(.callout).foregroundStyle(model.hasRemoteConnection ? .green : .secondary)
                        Spacer()
                        if !model.sharingEnabled {
                            Button("Turn On Sharing") { model.installService() }.disabled(model.busy || model.running == nil)
                        } else if model.peers.isEmpty {
                            Button("Connect a Mac") { model.setupRequested = true; model.panel = 4 }.buttonStyle(.borderedProminent)
                        } else {
                            Button("Manage Computers") { model.panel = 4 }
                        }
                    }
                }.padding(10)
            }
            if !model.peers.isEmpty {
                HStack {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Make it feel like one desk").font(.headline)
                        Text("Match the screen positions to your physical setup.").font(.callout).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Arrange Screens") { model.panel = 1 }
                }
            }
            if model.sharingEnabled && model.connected && (model.capture != "Enabled" || model.emulation != "Enabled") {
                GroupBox {
                    HStack {
                        Label("macOS needs permission to share input.", systemImage: "hand.raised")
                        Spacer()
                        Button("Review Permissions") { model.setupRequested = true; model.panel = 4 }
                    }.padding(8)
                }
            }
            if model.peer != nil {
                DisclosureGroup("Connection Health") {
                    VStack(alignment: .leading, spacing: 12) {
                        PeerPicker(model: model)
                        HStack(alignment: .firstTextBaseline) {
                            Text(model.samples.last?.ms.map { String(format: "%.1f ms", $0) } ?? "—").font(.title2).monospacedDigit()
                            Text("network round trip").font(.callout).foregroundStyle(.secondary)
                            Spacer()
                        }
                        if model.samples.isEmpty { Text("Waiting for network measurements.").foregroundStyle(.secondary) }
                        else {
                            Chart {
                                ForEach(model.samples) { sample in
                                    if let ms = sample.ms {
                                        PointMark(x: .value("Time", sample.date), y: .value("Round trip", ms)).foregroundStyle(.blue)
                                    } else {
                                        PointMark(x: .value("Time", sample.date), y: .value("Timeout", 0)).symbol(.cross).foregroundStyle(.red)
                                    }
                                }
                            }.chartYAxisLabel("ms").frame(height: 110)
                        }
                        Text("This measures the network, not the full delay of your mouse or keyboard. Red crosses mean a probe timed out.")
                            .font(.caption).foregroundStyle(.secondary)
                        DisclosureGroup("Network Details") {
                            LabeledContent("Route", value: model.transport)
                            LabeledContent("Address", value: model.peer?.address ?? "Not connected")
                            LabeledContent("Average round trip", value: ms(model.average))
                            LabeledContent("95th percentile", value: ms(model.p95))
                            LabeledContent("Probe loss", value: model.samples.isEmpty ? "—" : String(format: "%.0f%%", model.loss))
                        }.font(.caption)
                    }.padding(.top, 10)
                }
            }
            HStack {
                Text("Mouse or keyboard not behaving as expected?").font(.callout).foregroundStyle(.secondary)
                Spacer()
                Button("Get Help") { model.panel = 3 }.buttonStyle(.link)
            }
        }
    }
    func computer(_ name: String, local: Bool) -> some View {
        VStack(spacing: 8) {
            Image(systemName: local ? "laptopcomputer" : "display").font(.system(size: 38, weight: .light)).foregroundStyle(local ? .primary : .secondary)
            Text(name).font(.callout.weight(.medium)).lineLimit(2).multilineTextAlignment(.center)
            Text(local ? "This Mac" : "Other Mac").font(.caption).foregroundStyle(.secondary)
        }.frame(maxWidth: .infinity)
    }
    func ms(_ value: Double?) -> String { value.map { String(format: "%.1f ms", $0) } ?? "—" }
}

private extension View {
    func pageVisibility(_ visible: Bool) -> some View {
        opacity(visible ? 1 : 0).allowsHitTesting(visible).accessibilityHidden(!visible).zIndex(visible ? 1 : 0)
    }
}

struct InvitationBanner: View {
    @ObservedObject var model: ControlModel
    @ObservedObject var discovery: Discovery
    var body: some View {
        if let invitation = discovery.nearby.first(where: { model.canReviewInvitation($0) && model.invitationToReview != $0.invitationID }) {
            HStack {
                Image(systemName: "link").foregroundStyle(.blue)
                Text("\(invitation.name) wants to connect").font(.callout)
                Spacer()
                Button("Dismiss") { model.dismissInvitation(invitation) }
                    .help("Hide this request. A new invitation can still appear.")
                Button("Review") { model.invitationToReview = invitation.invitationID; model.panel = 4 }
            }.padding(12).background(.blue.opacity(0.08))
        }
    }
}

struct PeerPicker: View {
    @ObservedObject var model: ControlModel
    var body: some View {
        Picker("Computer", selection: $model.selected) {
            ForEach(model.peers) { Text(model.displayName(for: $0)).tag(Optional($0.id)) }
        }.frame(maxWidth: 420).disabled(!model.connected)
    }
}

struct Arrangement: View {
    @ObservedObject var model: ControlModel
    @ViewState private var pendingMove: (Int, String)?
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            PageHeading(title: "Match your desk", subtitle: "Place your other Mac where it sits beside this one. Move your pointer across that edge to switch.")
            if !model.connected {
                Label("Turn sharing on to change positions.", systemImage: "pause.circle").font(.callout).foregroundStyle(.secondary)
            } else if model.peers.isEmpty {
                Button("Connect a Mac First") { model.setupRequested = true; model.panel = 4 }
            }
            Grid(horizontalSpacing: 8, verticalSpacing: 8) {
                GridRow { Color.clear.frame(width: 150, height: 1); zone("top"); Color.clear.frame(width: 150, height: 1) }
                GridRow {
                    zone("left")
                    VStack(spacing: 10) {
                        Image(systemName: "desktopcomputer").font(.system(size: 38)).foregroundStyle(.blue)
                        Text("This Mac").font(.headline)
                        Text(model.machine).font(.caption).lineLimit(2)
                    }.frame(width: 150, height: 100).background(.blue.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                    zone("right")
                }
                GridRow { Color.clear.frame(width: 150, height: 1); zone("bottom"); Color.clear.frame(width: 150, height: 1) }
            }.frame(maxWidth: .infinity).padding(.vertical, 12)
            Text("Drag a computer between positions, or choose its edge below. Changes save automatically.").font(.caption).foregroundStyle(.secondary)
            Divider()
            ForEach(model.peers) { peer in
                HStack {
                    Label(model.displayName(for: peer), systemImage: "laptopcomputer").frame(maxWidth: .infinity, alignment: .leading)
                    Picker("Position", selection: Binding(get: { peer.position }, set: { requestMove(peer.id, $0) })) {
                        ForEach(["left", "right", "top", "bottom"], id: \.self) { Text($0.capitalized).tag($0) }
                    }.frame(width: 190)
                    Toggle("Send Input", isOn: Binding(get: { peer.active }, set: { model.send(["Activate": [peer.id, $0]]) })).toggleStyle(.switch)
                }.disabled(!model.connected || model.busy)
            }
            DisclosureGroup("How Screen Switching Works") {
                Text("One computer can receive input on each edge. Set the reverse direction on the other Mac so you can move back.")
                    .font(.caption).foregroundStyle(.secondary)
            }.font(.caption)
        }
        .alert("Replace the computer on this edge?", isPresented: Binding(get: { pendingMove != nil }, set: { if !$0 { pendingMove = nil } })) {
            Button("Cancel", role: .cancel) { pendingMove = nil }
            Button("Replace") {
                if let (id, position) = pendingMove { model.position(id, position) }
                pendingMove = nil
            }
        } message: {
            Text("The computer currently assigned to this edge will stop receiving input from this Mac. Its saved settings will stay.")
        }
    }
    func requestMove(_ id: Int, _ position: String) {
        guard model.connected, !model.busy else { return }
        if model.peers.contains(where: { $0.id != id && $0.active && $0.position == position }) { pendingMove = (id, position) }
        else { model.position(id, position) }
    }
    func zone(_ position: String) -> some View {
        VStack(spacing: 8) {
            Text(position == "top" ? "Above" : position == "bottom" ? "Below" : position.capitalized).font(.caption).foregroundStyle(.secondary)
            let peers = model.peers.filter { $0.position == position }
            if peers.isEmpty { Text("Drop here").font(.caption).foregroundStyle(.tertiary) }
            ForEach(peers) { peer in
                Label(model.displayName(for: peer), systemImage: "laptopcomputer").font(.callout).lineLimit(1)
                    .padding(9).background(peer.active ? Color.blue.opacity(0.12) : Color.gray.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
                    .draggable(String(peer.id))
            }
        }.frame(width: 150, height: 85)
            .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(.secondary.opacity(0.22), style: StrokeStyle(dash: [5])))
            .dropDestination(for: String.self) { items, _ in
                guard model.connected, let first = items.first, let id = Int(first), model.peers.contains(where: { $0.id == id }) else { return false }
                requestMove(id, position); return true
            }
    }
}

struct MappingPanel: View {
    @ObservedObject var model: ControlModel
    @ViewState private var draft = MappingDraft()
    @ViewState private var saving = false
    @ViewState private var message: String?
    @ViewState private var saveFailed = false
    @ViewState private var composer: MappingComposer?
    @ViewState private var pendingPeer: Int?
    @ViewState private var confirmSwitch = false
    var peer: Peer? { model.peers.first { $0.id == draft.peerID } }
    var conflict: Bool { peer.map { $0.mappings != draft.loaded && draft.dirty } ?? false }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            PageHeading(title: "Keys & buttons", subtitle: "Change inputs sent to your other Mac. Local controls stay the same.")
            if let peer {
                GroupBox {
                    HStack(spacing: 12) {
                        Image(systemName: "desktopcomputer").font(.title2).foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 3) {
                            Text("When controlling").font(.caption).foregroundStyle(.secondary)
                            if model.peers.count == 1 {
                                Text(model.displayName(for: peer)).font(.headline).lineLimit(1)
                            } else {
                                Picker("Computer", selection: Binding(get: { draft.peerID }, set: { requestSwitch($0) })) {
                                    ForEach(model.peers) { Text(model.displayName(for: $0)).tag(Optional($0.id)) }
                                }.labelsHidden().disabled(saving)
                            }
                            Text("From \(model.machine)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                        Spacer()
                        Label(!model.connected ? "Sharing off" : !peer.active ? "Sending paused" : peer.alive ? "Connected" : "Waiting",
                              systemImage: peer.alive && model.connected ? "checkmark.circle" : "pause.circle")
                            .font(.caption).foregroundStyle(.secondary)
                    }.padding(6)
                }
                if !model.connected {
                    HStack {
                        Text("You can edit a draft while sharing is off. Turn it on to save.").font(.callout).foregroundStyle(.secondary)
                        Spacer()
                        Button("Turn On Sharing") { model.installService() }.disabled(model.busy || model.running == nil)
                    }
                }
                if conflict {
                    Label("Saved rules changed elsewhere. Discard this draft to load the latest rules.", systemImage: "exclamationmark.triangle")
                        .font(.callout).foregroundStyle(.orange)
                }
                HStack {
                    Text("Custom rules").font(.headline)
                    if !draft.rows.isEmpty { Text("\(draft.rows.count)").font(.caption).foregroundStyle(.secondary) }
                    Spacer()
                    Menu {
                        Button("Keyboard Key…") { composer = MappingComposer(mouse: false) }
                        Button("Mouse Button…") { composer = MappingComposer(mouse: true) }
                    } label: { Label("Add Rule", systemImage: "plus") }
                        .fixedSize().disabled(saving)
                }
                ScrollView {
                    VStack(spacing: 8) {
                        if draft.rows.isEmpty {
                            VStack(spacing: 10) {
                                Image(systemName: "keyboard").font(.system(size: 28)).foregroundStyle(.secondary)
                                Text("No custom rules").font(.headline)
                                Text("Only the rules you add will change your inputs.\nFor example, make Caps Lock send Escape.")
                                    .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                                Button("Add Your First Rule…") { composer = MappingComposer(mouse: false) }
                            }.frame(maxWidth: .infinity).padding(.vertical, 24)
                        } else {
                            HStack {
                                Text("WHEN I PRESS").frame(maxWidth: .infinity, alignment: .leading)
                                Text("SEND INSTEAD").frame(maxWidth: .infinity, alignment: .leading)
                                Spacer().frame(width: 62)
                            }.font(.system(size: 10, weight: .semibold)).foregroundStyle(.secondary).padding(.horizontal, 12)
                            ForEach(draft.rows) { row in
                                HStack(spacing: 10) {
                                    Label(KeyChoice.label(row.from), systemImage: (272...276).contains(row.from) ? "computermouse" : "keyboard")
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                    Image(systemName: "arrow.right").foregroundStyle(.tertiary)
                                    Text(KeyChoice.label(row.to)).frame(maxWidth: .infinity, alignment: .leading)
                                    Button("Edit") { composer = MappingComposer(row: row) }.controlSize(.small)
                                    Button { draft.rows.removeAll { $0.id == row.id }; message = nil } label: {
                                        Image(systemName: "minus.circle").foregroundStyle(.secondary)
                                    }.buttonStyle(.borderless).help("Remove this rule")
                                        .accessibilityLabel("Remove \(KeyChoice.label(row.from)) rule")
                                }.font(.callout).padding(12)
                                    .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
                            }
                        }
                    }.disabled(saving)
                }.frame(maxHeight: .infinity)
                DisclosureGroup("What can I change?") {
                    Text("A key can send another key; a mouse button can send another button. Scrolling, gestures and multi-key shortcuts aren’t configured here. Rules apply only when sending from this Mac to the Mac above. Saving briefly releases shared input.")
                        .font(.caption).foregroundStyle(.secondary).padding(.top, 4)
                }.font(.caption)
                Divider()
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text(saving ? "Applying rules…" : draft.dirty ? "Unsaved changes" : "All changes saved")
                            .font(.callout.weight(.medium))
                        if let message { Text(message).font(.caption).foregroundStyle(saveFailed ? .orange : .secondary) }
                        else if draft.dirty { Text("Save to use these rules on your other Mac.").font(.caption).foregroundStyle(.secondary) }
                    }
                    Spacer()
                    Button("Discard") { load(peer) }.disabled(!draft.dirty || saving)
                    Button(saveFailed ? "Retry Save" : "Save Changes") { save(peer) }
                        .buttonStyle(.borderedProminent)
                        .disabled(!draft.dirty || draft.duplicate || !model.connected || saving || conflict)
                }
            } else if draft.dirty {
                ContentUnavailableView {
                    Label("This computer was removed", systemImage: "desktopcomputer.trianglebadge.exclamationmark")
                } description: { Text("Your draft is still here, but it can’t be applied to another computer.") }
                actions: { Button("Discard Draft") { draft = MappingDraft(); if let p = model.peer { load(p) } } }
            } else {
                ContentUnavailableView {
                    Label("Add your other Mac", systemImage: "keyboard")
                } description: { Text("Connect a Mac first, then customize what its keys and mouse buttons receive.") }
                actions: { Button("Go to Computers") { model.panel = 4 } }
            }
        }
        .sheet(item: $composer) { item in
            MappingRuleSheet(composer: item, usedSources: Set(draft.rows.filter { $0.id != item.rowID }.map(\.from))) { row in
                if let index = draft.rows.firstIndex(where: { $0.id == row.id }) { draft.rows[index] = row }
                else { draft.rows.append(row) }
                message = nil; saveFailed = false
            }
        }
        .confirmationDialog("Discard unsaved rules before switching computers?", isPresented: $confirmSwitch, titleVisibility: .visible) {
            Button("Discard and Switch", role: .destructive) {
                if let p = model.peers.first(where: { $0.id == pendingPeer }) { load(p) }
                pendingPeer = nil
            }
            Button("Keep Editing", role: .cancel) { pendingPeer = nil }
        }
        .onAppear { if draft.peerID == nil, let p = model.peer { load(p) } }
        .onChange(of: model.requestedControlsPeer) { _, id in
            guard id != nil else { return }
            model.requestedControlsPeer = nil; requestSwitch(id)
        }
        .onChange(of: model.peers) { _, _ in
            if !saving, saveFailed, let p = peer, draft.matchesSaved(p) {
                load(p); message = "Saved. Your rules are ready to use."
            } else if !draft.dirty && !saving {
                if let p = peer ?? model.peer, draft.peerID != p.id || draft.loaded != p.mappings { load(p) }
            }
        }
    }
    func requestSwitch(_ id: Int?) {
        guard id != draft.peerID, !saving, let p = model.peers.first(where: { $0.id == id }) else { return }
        if draft.dirty { pendingPeer = id; confirmSwitch = true }
        else { load(p) }
    }
    func load(_ peer: Peer) { draft.load(peer); message = nil; saveFailed = false }
    func save(_ peer: Peer) {
        guard !draft.duplicate, !conflict else { return }
        saving = true; message = nil; saveFailed = false; model.error = nil
        let expected = draft.dictionary
        model.mappings(peer.id, expected)
        Task { @MainActor in
            let deadline = Date().addingTimeInterval(5)
            while Date() < deadline && model.connected && model.error == nil {
                if model.peers.first(where: { $0.id == peer.id })?.mappings == expected { break }
                try? await Task.sleep(nanoseconds: 30_000_000)
            }
            if let current = model.peers.first(where: { $0.id == peer.id }), current.mappings == expected, model.error == nil {
                load(current); message = "Ready to use on \(model.displayName(for: current))."
            } else { saveFailed = true; message = "Couldn’t confirm the save. Your draft is safe; try again." }
            saving = false
        }
    }
}

struct MappingComposer: Identifiable {
    let id = UUID()
    var rowID: UUID?
    var mouse = false
    var from: Int?
    var to: Int?
    init(mouse: Bool) { self.mouse = mouse }
    init(row: MappingRow) { rowID = row.id; mouse = (272...276).contains(row.from); from = row.from; to = row.to }
}

struct MappingRuleSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ViewState var composer: MappingComposer
    let usedSources: Set<Int>
    let apply: (MappingRow) -> Void
    var options: [KeyChoice] { composer.mouse ? KeyChoice.buttons : KeyChoice.all }
    var valid: Bool { composer.from != nil && composer.to != nil && composer.from != composer.to && !usedSources.contains(composer.from ?? -1) }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text(composer.rowID == nil ? "Add a rule" : "Edit rule").font(.title2.weight(.semibold))
            if composer.rowID == nil {
                Picker("Input", selection: $composer.mouse) {
                    Text("Keyboard key").tag(false); Text("Mouse button").tag(true)
                }.pickerStyle(.segmented)
                    .onChange(of: composer.mouse) { _, _ in composer.from = nil; composer.to = nil }
            }
            Form {
                Picker("When I press", selection: $composer.from) {
                    Text(composer.mouse ? "Choose a button…" : "Choose a key…").tag(Optional<Int>.none)
                    ForEach(options.filter { !usedSources.contains($0.id) }) { Text($0.name).tag(Optional($0.id)) }
                    if let value = composer.from, !options.contains(where: { $0.id == value }) { Text(KeyChoice.label(value)).tag(Optional(value)) }
                }
                Picker("Send instead", selection: $composer.to) {
                    Text(composer.mouse ? "Choose a button…" : "Choose a key…").tag(Optional<Int>.none)
                    ForEach(options) { Text($0.name).tag(Optional($0.id)) }
                    if let value = composer.to, !options.contains(where: { $0.id == value }) { Text(KeyChoice.label(value)).tag(Optional(value)) }
                }
            }
            Text("This adds to your draft. Your controls change only after you save.")
                .font(.caption).foregroundStyle(.secondary)
            if composer.from != nil && composer.from == composer.to {
                Text("Choose a different output, or leave this input unchanged without a rule.").font(.caption).foregroundStyle(.orange)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(composer.rowID == nil ? "Add to Draft" : "Update Draft") {
                    guard let from = composer.from, let to = composer.to, valid else { return }
                    apply(MappingRow(id: composer.rowID ?? UUID(), from: from, to: to)); dismiss()
                }.buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction).disabled(!valid)
            }
        }.padding(24).frame(width: 420)
    }
}

struct DebugPanel: View {
    @ObservedObject var model: ControlModel
    @ViewState private var pane = 0
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                PageHeading(title: "Get back to sharing", subtitle: "Check input, reconnect, or collect details when something goes wrong.")
                Spacer()
                Button("Export Report…", action: model.exportReport)
            }
            GroupBox {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Label(model.connected ? "Sharing service is running" : "Sharing service is unavailable", systemImage: model.connected ? "checkmark.circle" : "exclamationmark.circle")
                        Spacer()
                        Button("Restart Sharing") { model.restart() }.disabled(model.busy || model.running != true)
                    }
                    if model.connected {
                        HStack {
                            Text("Send input: \(model.capture == "Enabled" ? "Ready" : "Needs attention") · Receive input: \(model.emulation == "Enabled" ? "Ready" : "Needs attention")")
                                .font(.caption).foregroundStyle(.secondary)
                            Spacer()
                            Button("Check Again") { model.send("EnableCapture"); model.send("EnableEmulation") }.controlSize(.small)
                        }
                    }
                    HStack {
                        Text("Can’t find the other Mac? Check the same network and Local Network access on both.").font(.caption).foregroundStyle(.secondary)
                        Spacer()
                        Button("Setup Help") { model.setupRequested = true; model.panel = 4 }.controlSize(.small)
                    }
                }.padding(8)
            }
            Picker("Diagnostics", selection: $pane) {
                Text("Test Input").tag(0); Text("Connection Events").tag(1); Text("Service Log").tag(2)
            }.pickerStyle(.segmented).labelsHidden()
            if pane == 0 {
                Text("Click the pad, then try keys, buttons, or scrolling. To inspect forwarded input, open this pad on the receiving Mac. It reports macOS events, not the escape sequences produced by a terminal.")
                    .font(.callout).foregroundStyle(.secondary)
                InputPad(enabled: model.panel == 3 && pane == 0) { model.recordInput($0) }.frame(height: 145)
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(.blue.opacity(0.4)))
                HStack { Text("Input events · only captured while this pad has focus").font(.caption).foregroundStyle(.secondary); Spacer(); Button("Clear") { model.debugEvents = [] } }
                ScrollView { Text(model.debugEvents.joined(separator: "\n")).font(.system(.caption, design: .monospaced)).frame(maxWidth: .infinity, alignment: .leading).textSelection(.enabled) }
            } else {
                ScrollView {
                    Text(pane == 1 ? model.traces.map { "\($0.date.formatted(date: .omitted, time: .standard))  \($0.text)" }.joined(separator: "\n") : model.logs)
                        .font(.system(.caption, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                }
                Text("Input test events are kept only in memory and are excluded from exported reports.").font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

struct InputPad: NSViewRepresentable {
    let enabled: Bool
    let event: (String) -> Void
    func makeNSView(context: Context) -> TestPad { let view = TestPad(); view.enabled = enabled; view.event = event; return view }
    func updateNSView(_ view: TestPad, context: Context) {
        view.event = event; view.enabled = enabled
        if !enabled && view.window?.firstResponder === view { view.window?.makeFirstResponder(nil) }
    }
}
final class TestPad: NSView {
    var event: ((String) -> Void)?
    var enabled = false
    override var acceptsFirstResponder: Bool { enabled }
    override func becomeFirstResponder() -> Bool { needsDisplay = true; return true }
    override func resignFirstResponder() -> Bool { needsDisplay = true; return true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.controlBackgroundColor.setFill(); bounds.fill()
        let text = window?.firstResponder === self ? "Listening here · press keys, click, or scroll" : "Click here to test input"
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 15, weight: .medium), .foregroundColor: NSColor.secondaryLabelColor]
        let size = text.size(withAttributes: attributes)
        text.draw(at: NSPoint(x: (bounds.width-size.width)/2, y: (bounds.height-size.height)/2), withAttributes: attributes)
    }
    func report(_ name: String, _ e: NSEvent) {
        guard enabled, window?.firstResponder === self, window?.isKeyWindow == true else { return }
        let flags = e.modifierFlags
        let mods: [(NSEvent.ModifierFlags, String)] = [(.command,"Cmd"),(.option,"Option"),(.control,"Ctrl"),(.shift,"Shift"),(.function,"Fn")]
        let label = mods.filter { flags.contains($0.0) }.map(\.1).joined(separator: "+")
        event?("\(name)  modifiers=\(label.isEmpty ? "none" : label)  flags=0x\(String(flags.rawValue, radix: 16))")
    }
    override func keyDown(with e: NSEvent) { report("Key down · code \(e.keyCode)\(e.isARepeat ? " · repeat" : "")", e) }
    override func keyUp(with e: NSEvent) { report("Key up · code \(e.keyCode)", e) }
    override func flagsChanged(with e: NSEvent) { report("Modifier · code \(e.keyCode)", e) }
    override func mouseDown(with e: NSEvent) { window?.makeFirstResponder(self); report("Mouse \(e.buttonNumber) down", e) }
    override func mouseUp(with e: NSEvent) { report("Mouse \(e.buttonNumber) up", e) }
    override func rightMouseDown(with e: NSEvent) { window?.makeFirstResponder(self); report("Mouse \(e.buttonNumber) down", e) }
    override func rightMouseUp(with e: NSEvent) { report("Mouse \(e.buttonNumber) up", e) }
    override func otherMouseDown(with e: NSEvent) { window?.makeFirstResponder(self); report("Mouse \(e.buttonNumber) down", e) }
    override func otherMouseUp(with e: NSEvent) { report("Mouse \(e.buttonNumber) up", e) }
    override func scrollWheel(with e: NSEvent) { report(String(format:"Scroll x=%.1f y=%.1f", e.scrollingDeltaX, e.scrollingDeltaY), e) }
}
