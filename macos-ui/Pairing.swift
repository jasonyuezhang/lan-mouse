import SwiftUI
import AppKit

struct PairingPanel: View {
    @ObservedObject var model: ControlModel
    @ObservedObject private var discovery: Discovery
    @ViewState private var stage = 0
    @ViewState private var host = ""
    @ViewState private var remoteFingerprint = ""
    @ViewState private var port = 4242
    @ViewState private var position = "left"
    @ViewState private var verified = false
    @ViewState private var selectedDiscovery: String?
    @ViewState private var invitationID = ""
    @ViewState private var incoming = false
    @ViewState private var manual = false
    @ViewState private var otherName = "the other Mac"
    @ViewState private var removePeer: Peer?
    @ViewState private var revokeFingerprint = ""
    @ViewState private var confirmedIncoming: String?
    @ViewState private var removeTrust: String?
    @ViewState private var initialized = false
    @ViewState private var loadedComputers = false
    init(model: ControlModel) { self.model = model; self.discovery = model.discovery }
    var ready: Bool { model.connected && model.capture == "Enabled" && model.emulation == "Enabled" }
    var selectedMac: NearbyMac? { discovery.nearby.first { $0.id == selectedDiscovery && $0.fingerprint == remoteFingerprint && $0.host == host && $0.port == port } }
    var pairingPeer: Peer? { model.peers.first { $0.matches(host: host, port: port) } }
    var configured: Bool { model.connected && model.authorized[remoteFingerprint] != nil && pairingPeer?.active == true && pairingPeer?.position == position && !model.pairing && model.error == nil }
    var remoteProgress: String? { selectedMac?.progress(for: invitationID, target: model.fingerprint) }
    var invitationValid: Bool {
        guard let item = selectedMac else { return false }
        return (!incoming && discovery.invitationIsActive(invitationID)) || (incoming && item.invitationID == invitationID && item.invitationTarget == model.fingerprint && oppositePosition(item.invitationPosition ?? "") == position)
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                PageHeading(title: stage == 5 ? "Your computers" : "Connect a Mac",
                            subtitle: stage == 5 ? "Choose which Macs can share your desk." : "A few steps, on both computers.")
                Spacer()
                Button(stage == 5 ? "Add Mac…" : "Your Computers") { stage = stage == 5 ? 0 : 5 }
            }
            if stage < 4 {
                HStack(spacing: 8) {
                    ForEach(Array(["Prepare", "Choose Mac", "Verify", "Try It"].enumerated()), id: \.offset) { index, title in
                        HStack(spacing: 5) {
                            Image(systemName: index < stage ? "checkmark.circle.fill" : "\(index + 1).circle\(index == stage ? ".fill" : "")")
                            Text(title)
                        }.font(.caption.weight(index == stage ? .semibold : .regular))
                            .foregroundStyle(index <= stage ? Color.accentColor : Color.secondary)
                        if index < 3 { Rectangle().fill(.quaternary).frame(height: 1) }
                    }
                }.padding(.vertical, 8)
            }
            if stage == 0 { readiness }
            else if stage == 1 { chooseComputer }
            else if stage == 2 { comparison }
            else if stage == 3 { trySharing }
            else if stage == 4 {
                Label("You’re ready to share", systemImage: "checkmark.circle.fill").font(.title2).foregroundStyle(.green)
                Text("Move across the edge to switch computers. Use the mouse icon in the menu bar to pause sharing anytime.")
                Text("Mouse settings can follow you across both Macs. In Mouse, keep ‘Use the same settings on both Macs’ enabled on each computer; changes sync while connected.")
                    .font(.callout).foregroundStyle(.secondary)
                Button("Done") { stage = 5; model.panel = 0 }.buttonStyle(.borderedProminent)
            } else { management }
            if stage < 3 { advanced }
        }
        .onAppear {
            if !initialized { stage = model.peers.isEmpty ? 0 : 5; loadedComputers = !model.peers.isEmpty; initialized = true }
            if model.setupRequested { stage = 0; model.setupRequested = false }
            reviewInvitation()
        }
        .onChange(of: model.peers.isEmpty) { _, empty in
            if !empty && !loadedComputers {
                loadedComputers = true
                if stage == 0 && !model.setupRequested { stage = 5 }
            }
        }
        .onChange(of: model.setupRequested) { _, requested in
            if requested { stage = 0; model.setupRequested = false }
        }
        .onChange(of: model.panel) { _, panel in
            if panel == 4 && stage == 4 { stage = 5 }
        }
        .onChange(of: configured) { _, _ in publishApproval() }
        .onChange(of: stage) { _, _ in publishApproval() }
        .onChange(of: model.invitationToReview) { _, _ in reviewInvitation() }
        .sheet(item: $removePeer) { peer in
            let identity = model.verifiedIdentity(for: peer) ?? revokeFingerprint
            VStack(alignment: .leading, spacing: 16) {
                Text("Remove \(model.displayName(for: peer))?").font(.title2.bold())
                Text("This removes its saved position and key mappings from this Mac.")
                if model.verifiedIdentity(for: peer) != nil {
                    Text("It will also stop this computer from controlling this Mac. Repeat Remove on the other Mac to clear its saved connection too.").font(.callout).foregroundStyle(.secondary)
                } else if !model.authorized.isEmpty {
                    Text("This older connection has no saved identity link. Choose its name below to also stop it from controlling this Mac.").font(.callout).foregroundStyle(.secondary)
                    Picker("Remove access for", selection: $revokeFingerprint) {
                        Text("Keep existing access").tag("")
                        ForEach(model.authorized.keys.sorted(), id: \.self) { fp in Text(model.authorized[fp] ?? "Computer").tag(fp) }
                    }
                }
                if !identity.isEmpty {
                    DisclosureGroup("Identity details") {
                        Text(identity).font(.caption.monospaced()).textSelection(.enabled)
                    }
                    Text("Sharing will briefly restart to finish removing access.").font(.caption).foregroundStyle(.secondary)
                }
                HStack {
                    Spacer(); Button("Cancel") { removePeer = nil }
                    Button("Remove Mac", role: .destructive) {
                        model.disconnectPeer(peer, forget: true, revoke: identity); removePeer = nil
                    }.buttonStyle(.borderedProminent).disabled(!model.connected || model.busy)
                }
            }.padding(24).frame(width: 500)
        }
        .alert("Allow this computer to control this Mac?", isPresented: Binding(get: { confirmedIncoming != nil }, set: { if !$0 { confirmedIncoming = nil } })) {
            Button("Cancel", role: .cancel) { confirmedIncoming = nil }
            Button("Fingerprints match — allow") {
                if let fp = confirmedIncoming { model.send(["AuthorizeKey": ["Paired computer", fp]]); model.pendingFingerprints.remove(fp) }
                confirmedIncoming = nil
            }
        } message: { Text("Compare this complete fingerprint with the other computer’s panel before allowing it:\n\n\(confirmedIncoming ?? "")") }
        .alert("Remove this computer’s access?", isPresented: Binding(get: { removeTrust != nil }, set: { if !$0 { removeTrust = nil } })) {
            Button("Cancel", role: .cancel) { removeTrust = nil }
            Button("Remove Access", role: .destructive) { if let fp = removeTrust { model.revoke(fp) }; removeTrust = nil }
        } message: { Text("This computer will no longer be allowed to control this Mac. Sharing briefly restarts to close existing sessions.\n\n\(removeTrust ?? "")") }
    }
    var readiness: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Use one mouse and keyboard on both Macs.").font(.headline)
            Text("Open Lan Mouse Control on the other Mac and connect both to the same Wi-Fi or wired network.").foregroundStyle(.secondary)
            Label(model.connected ? "Sharing is running on this Mac" : "Turn on sharing to check this Mac", systemImage: model.connected ? "checkmark.circle.fill" : "power")
            if !model.connected {
                Button(model.busy ? "Starting…" : "Get started") { model.installService() }.disabled(model.busy)
            }
            if model.connected {
            permissionRow("Let the other Mac control this one", explanation: "Accessibility lets this Mac receive mouse and keyboard input.", enabled: model.emulation == "Enabled", setting: "Privacy_Accessibility")
            permissionRow("Share your mouse and keyboard", explanation: "Input Monitoring lets Lan Mouse send your input to the other Mac.", enabled: model.capture == "Enabled", setting: "Privacy_ListenEvent")
            if !ready {
                Text("In Settings, turn on lan-mouse. If it isn’t listed, drag the highlighted file from Finder into the permission list.").font(.caption).foregroundStyle(.secondary)
                Button("Show Lan Mouse in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: NSHomeDirectory() + "/.local/bin/lan-mouse")]) }
                Button("I’ve allowed access — check again") { model.send("EnableCapture"); model.send("EnableEmulation") }.disabled(!model.connected)
            }
            }
            Button("Find My Other Mac") { stage = 1 }.buttonStyle(.borderedProminent).disabled(!ready)
        }
    }
    func permissionRow(_ title: String, explanation: String, enabled: Bool, setting: String) -> some View {
        HStack {
            Image(systemName: enabled ? "checkmark.circle.fill" : "circle").foregroundStyle(enabled ? .green : .secondary)
            VStack(alignment: .leading, spacing: 3) { Text(title); Text(explanation).font(.caption).foregroundStyle(.secondary) }
            Spacer()
            if !enabled { Button("Open Settings") { NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?" + setting)!) } }
        }.padding(10).background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 8))
    }
    var chooseComputer: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Which Mac would you like to connect?").font(.headline)
            Text("Open Lan Mouse on the other Mac. When you continue, it will show an invitation you can review from any page.").font(.callout).foregroundStyle(.secondary)
            if discovery.nearby.isEmpty {
                Label("Looking for your other Mac…", systemImage: "magnifyingglass")
                Text("Check that both apps are open and both Macs use the same network. If macOS asks, allow Local Network access.").font(.callout).foregroundStyle(.secondary)
                Button("Open Local Network settings") { NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")!) }
            }
            if let problem = discovery.problem { Text(problem).font(.caption).foregroundStyle(.orange) }
            ForEach(discovery.nearby) { item in
                Button {
                    select(item)
                    if model.canReviewInvitation(item),
                       let inverse = oppositePosition(item.invitationPosition ?? ""),
                       let id = item.invitationID, UUID(uuidString: id) != nil {
                        position = inverse; invitationID = id; incoming = true; stage = 2
                        discovery.reportProgress("reviewing", invitation: id, target: remoteFingerprint)
                    }
                } label: {
                    HStack {
                        Image(systemName: "laptopcomputer")
                        Text(item.name)
                        Spacer()
                        if model.canReviewInvitation(item) { Text("Wants to connect · Review").font(.caption) }
                        else if selectedDiscovery == item.id { Image(systemName: "checkmark.circle.fill") }
                    }.padding(8).frame(maxWidth: .infinity)
                }.buttonStyle(.bordered)
            }
            if selectedMac != nil {
                Text("Where is \(otherName) on your desk?")
                deskPosition
                Button("Continue") {
                    incoming = false
                    invitationID = discovery.invite(remoteFingerprint, position: position)
                    stage = 2
                }.buttonStyle(.borderedProminent)
            }
            Button("Back") { stage = 0 }
        }
    }
    func select(_ item: NearbyMac) {
        manual = false
        host = item.host; remoteFingerprint = item.fingerprint; port = item.port
        selectedDiscovery = item.id; otherName = item.name; verified = false
    }
    var comparison: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Make sure you’re connecting your two Macs.").font(.headline)
            Text(incoming ? "Compare this code with the one shown on \(otherName)." : "On \(otherName), click Review in the connection invitation. Compare the code on both screens.")
            if let code = pairingCode(model.fingerprint, remoteFingerprint, invitation: invitationID) {
                Text(code).font(.system(size: 22, weight: .semibold, design: .monospaced)).textSelection(.enabled).padding(12).frame(maxWidth: .infinity).background(.quaternary, in: RoundedRectangle(cornerRadius: 10))
            }
            Label(otherName, systemImage: "desktopcomputer").font(.callout.weight(.medium))
            if !incoming {
                Label(remoteProgress == "approved" ? "Approved on \(otherName)" : remoteProgress == "reviewing" ? "The code is open on \(otherName)" : "Waiting for \(otherName) to open the invitation…",
                      systemImage: remoteProgress == nil ? "clock" : "checkmark.circle")
                    .font(.callout).foregroundStyle(.secondary)
                Button("Send invitation again") { discovery.resendInvitation() }
                    .controlSize(.small).disabled(!invitationValid)
            }
            Text("Only continue if every group matches. This allows the Macs to share mouse and keyboard input.").font(.callout).foregroundStyle(.secondary)
            if !ready {
                Text("This Mac needs permission before it can connect.").font(.callout).foregroundStyle(.secondary)
                Button("Review Permissions") { stage = 0 }
            }
            if !invitationValid { Text("The other Mac is no longer available or the invitation changed. Go back and choose it again.").foregroundStyle(.orange) }
            HStack {
                Button(incoming ? "Dismiss" : "Back") {
                    if incoming, let item = selectedMac {
                        model.dismissInvitation(item); stage = 5
                    } else { discovery.cancelInvitation(); stage = 1 }
                }
                Spacer()
                Button("The codes match — connect") {
                    model.pair(PairRequest(host: host, port: port, position: position, fingerprint: remoteFingerprint)); stage = 3
                }.buttonStyle(.borderedProminent).disabled(!ready || !invitationValid || model.pairing)
            }
        }
    }
    var trySharing: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Now try your mouse").font(.headline)
            if !configured {
                Label("Finishing setup on this Mac…", systemImage: "clock")
                if model.error != nil {
                    Button("Retry setup") { model.pair(PairRequest(host: host, port: port, position: position, fingerprint: remoteFingerprint)) }
                }
            } else if manual {
                Text("Complete setup on the other Mac too, choosing the opposite position.")
            } else if pairingPeer?.alive == true || remoteProgress == "approved" {
                Label("Both Macs are ready", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                Text("Use your mouse or trackpad to try the crossing below.")
            } else {
                Label("Waiting for approval on \(otherName)", systemImage: "clock")
                Text("On \(otherName), compare the code and click ‘The codes match — connect’. This screen will update when it’s ready.").font(.callout).foregroundStyle(.secondary)
            }
            Text("Move the pointer past this Mac’s \(position == "top" ? "top" : position == "bottom" ? "bottom" : position) edge, toward \(otherName). Then move back.").font(.title3)
            Label(pairingPeer?.alive == true ? "The other Mac is responding" : (configured ? "Paired here · waiting for your first mouse crossing" : "Waiting for setup to finish"), systemImage: pairingPeer?.alive == true ? "checkmark.circle.fill" : "arrow.left.arrow.right")
            Text("To return, move across the facing edge on the other Mac. If the pointer gets stuck, hold Control + Shift + Command + Option together on the keyboard you started with.").font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("Back to setup") { discovery.cancelInvitation(); stage = 1 }
                Spacer()
                Button("It works!") {
                    stage = 4
                }.buttonStyle(.borderedProminent).disabled(!configured || pairingPeer?.alive != true)
            }
        }
    }
    var advanced: some View {
        DisclosureGroup("Advanced connection settings") {
            VStack(alignment: .leading, spacing: 10) {
                Text("This Mac").font(.headline)
                Text("\(Host.current().name ?? model.machine) · Port \(String(model.port))").font(.caption).textSelection(.enabled)
                Text(model.fingerprint).font(.caption.monospaced()).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                Button("Copy this Mac’s fingerprint") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(model.fingerprint, forType: .string) }
                if stage == 1 {
                    Divider()
                    Text("Connect manually").font(.headline)
                    Text("Use this if nearby discovery is unavailable. Compare the full fingerprint using Advanced on the other Mac, then repeat manually there with the opposite position.").font(.caption).foregroundStyle(.secondary)
                    TextField("Hostname or IP address", text: $host).textFieldStyle(.roundedBorder)
                    TextField("Other Mac’s fingerprint", text: $remoteFingerprint).textFieldStyle(.roundedBorder).font(.caption.monospaced())
                    HStack {
                        TextField("Port", value: $port, format: .number.grouping(.never)).frame(width: 90)
                        Picker("Position", selection: $position) { ForEach(["left", "right", "top", "bottom"], id: \.self) { Text($0.capitalized).tag($0) } }
                    }
                    Toggle("I compared the full fingerprint on the other Mac", isOn: $verified)
                    Button("Connect manually") {
                        host = host.trimmingCharacters(in: .whitespacesAndNewlines)
                        remoteFingerprint = normalizedFingerprint(remoteFingerprint) ?? remoteFingerprint
                        otherName = host; manual = true
                        model.pair(PairRequest(host: host, port: port, position: position, fingerprint: remoteFingerprint)); stage = 3
                    }.disabled(!ready || !verified || normalizedFingerprint(remoteFingerprint) == nil || host.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !(1...65535).contains(port))
                    .onChange(of: remoteFingerprint) { _, _ in verified = false }
                    .onChange(of: host) { _, _ in verified = false }
                }
            }.padding(.top, 8)
        }.font(.callout).padding(.top, 8)
    }
    func reviewInvitation() {
        guard let id = model.invitationToReview,
              let item = discovery.nearby.first(where: { $0.invitationID == id && model.canReviewInvitation($0) }),
              let inverse = oppositePosition(item.invitationPosition ?? "") else { return }
        select(item); position = inverse; invitationID = id; incoming = true; stage = 2
        discovery.reportProgress("reviewing", invitation: id, target: remoteFingerprint)
    }
    func publishApproval() {
        guard stage == 3, configured, !manual else { return }
        discovery.reportProgress("approved", invitation: invitationID, target: remoteFingerprint)
    }
    var deskPosition: some View {
        Grid(horizontalSpacing: 6, verticalSpacing: 6) {
            GridRow { Color.clear.frame(width: 130, height: 1); positionButton("top", title: "Above"); Color.clear.frame(width: 130, height: 1) }
            GridRow {
                positionButton("left", title: "Left")
                VStack(spacing: 4) {
                    Image(systemName: "laptopcomputer").font(.title2)
                    Text("This Mac").font(.caption.weight(.semibold))
                }.frame(width: 130, height: 62).background(.quaternary, in: RoundedRectangle(cornerRadius: 8))
                positionButton("right", title: "Right")
            }
            GridRow { Color.clear.frame(width: 130, height: 1); positionButton("bottom", title: "Below"); Color.clear.frame(width: 130, height: 1) }
        }.frame(maxWidth: .infinity)
    }
    func positionButton(_ edge: String, title: String) -> some View {
        Button { position = edge } label: {
            VStack(spacing: 3) {
                Image(systemName: position == edge ? "laptopcomputer" : "plus").font(.title3)
                Text(position == edge ? otherName : title).font(.caption).lineLimit(1)
            }.frame(width: 130, height: 54).contentShape(Rectangle())
        }.buttonStyle(.plain)
            .background(position == edge ? Color.accentColor.opacity(0.15) : Color.secondary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(position == edge ? Color.accentColor : Color.secondary.opacity(0.2)))
            .accessibilityLabel("Place \(otherName) \(title.lowercased()) of this Mac")
            .accessibilityAddTraits(position == edge ? .isSelected : [])
    }
    var management: some View {
        VStack(alignment: .leading, spacing: 16) {
            if !model.connected {
                GroupBox {
                    HStack {
                        Text(model.sharingEnabled ? "Reconnecting to sharing…" : "Turn sharing on to manage connections and access.").font(.callout)
                        Spacer()
                        if !model.sharingEnabled { Button("Turn On") { model.installService() }.disabled(model.busy || model.running == nil) }
                    }.padding(8)
                }
            }
            if model.peers.isEmpty {
                VStack(spacing: 10) {
                    Image(systemName: "display.2").font(.largeTitle).foregroundStyle(.secondary)
                    Text(model.connected ? "Your other Mac belongs here" : model.sharingEnabled ? "Refreshing your computers…" : "Connections appear when sharing is on").font(.headline)
                    Text("Connect a Mac once, then switch by moving across the screen edge.").font(.callout).foregroundStyle(.secondary)
                    if model.connected { Button("Connect a Mac") { stage = 0 }.buttonStyle(.borderedProminent) }
                }.frame(maxWidth: .infinity).padding(20)
            }
            ForEach(model.peers) { peer in
                GroupBox {
                    VStack(alignment: .leading, spacing: 12) {
                        HStack(spacing: 12) {
                            Image(systemName: "desktopcomputer").font(.title2).foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 4) {
                                Text(model.displayName(for: peer)).font(.headline)
                                Text(!model.connected ? "Sharing is off" : !peer.active ? "Sending paused" : peer.alive ? "Connected · \(peer.position.capitalized) edge" : "Not connected · \(peer.position.capitalized) edge")
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Spacer()
                            if !peer.active {
                                Button("Resume") { model.send(["Activate": [peer.id, true]]) }
                                    .disabled(!model.connected || model.busy)
                            }
                            Menu {
                                Button("Arrange Screens") { model.selected = peer.id; model.panel = 1 }
                                Button("Keys & Buttons…") { model.requestedControlsPeer = peer.id; model.panel = 2 }
                                Divider()
                                if peer.active {
                                    Button("Pause Sending to This Mac") { model.send(["Activate": [peer.id, false]]) }
                                        .disabled(!model.connected || model.busy)
                                }
                                Button("Remove Mac…", role: .destructive) { revokeFingerprint = ""; removePeer = peer }
                                    .disabled(!model.connected || model.busy)
                            } label: { Image(systemName: "ellipsis") }.menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                                .accessibilityLabel("Options for \(model.displayName(for: peer))")
                        }
                    }.padding(10)
                }
            }
            DisclosureGroup("Who Can Control This Mac") {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Allowed computers can send mouse and keyboard input to this Mac. Connections paired here link this access to Remove Mac. Older connections may need access removed separately.")
                        .font(.caption).foregroundStyle(.secondary)
                    if model.authorized.isEmpty { Text("No allowed computers reported.").foregroundStyle(.secondary) }
                    ForEach(model.authorized.keys.sorted(), id: \.self) { fp in
                        HStack(alignment: .top) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(model.authorized[fp] ?? "Computer").font(.callout.weight(.medium))
                                DisclosureGroup("Identity") {
                                    Text(fp).font(.caption2.monospaced()).textSelection(.enabled)
                                }.font(.caption)
                            }
                            Spacer()
                            Button("Remove Access…") { removeTrust = fp }.disabled(!model.connected || model.busy)
                        }
                    }
                    Text("These changes apply to this Mac. To remove access in both directions, repeat on the other Mac.")
                        .font(.caption).foregroundStyle(.secondary)
                }.padding(.top, 8)
            }
        }
    }
}
