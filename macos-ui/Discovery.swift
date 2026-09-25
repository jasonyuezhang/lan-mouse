import SwiftUI
import Foundation
import AppKit
import Darwin
import CryptoKit

struct NearbyMac: Identifiable, Equatable {
    let id: String
    let name: String
    let host: String
    let ips: [String]
    let port: Int
    let fingerprint: String
    var invitationTarget: String? = nil
    var invitationPosition: String? = nil
    var invitationID: String? = nil
    var progressTarget: String? = nil
    var progressID: String? = nil
    var progressStage: String? = nil

    func progress(for invitation: String, target: String) -> String? {
        guard !invitation.isEmpty, progressID == invitation, progressTarget == target else { return nil }
        return progressStage
    }
}

func normalizedFingerprint(_ value: String) -> String? {
    let hex = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().replacingOccurrences(of: ":", with: "")
    guard hex.count == 64, hex.allSatisfy({ "0123456789abcdef".contains($0) }) else { return nil }
    return stride(from: 0, to: 64, by: 2).map { offset in
        let a = hex.index(hex.startIndex, offsetBy: offset), b = hex.index(a, offsetBy: 2)
        return String(hex[a..<b])
    }.joined(separator: ":")
}

func oppositePosition(_ position: String) -> String? {
    ["left": "right", "right": "left", "top": "bottom", "bottom": "top"][position]
}

// Bind the comparison to both full identities and this invitation. 96 bits are
// displayed; discovery itself never authorizes a computer.
func pairingCode(_ first: String, _ second: String, invitation: String) -> String? {
    guard let a = normalizedFingerprint(first), let b = normalizedFingerprint(second), a != b,
          UUID(uuidString: invitation) != nil else { return nil }
    let input = ([a, b].sorted() + [invitation]).joined(separator: "|")
    let hex = SHA256.hash(data: Data(input.utf8)).prefix(12).map { String(format: "%02X", $0) }.joined()
    return stride(from: 0, to: hex.count, by: 4).map { offset in
        let start = hex.index(hex.startIndex, offsetBy: offset)
        return String(hex[start..<hex.index(start, offsetBy: 4)])
    }.joined(separator: " ")
}

@MainActor final class Discovery: NSObject, ObservableObject, @preconcurrency NetServiceBrowserDelegate, @preconcurrency NetServiceDelegate {
    @Published var nearby: [NearbyMac] = []
    @Published var problem: String?
    private let browser = NetServiceBrowser()
    private var services: [NetService] = []
    private var advertisement: NetService?
    private var ownFingerprint = ""
    private var started = false
    private var lastRecords: [String: Data]?
    private var invitation: (target: String, position: String, id: String, expires: Date)?
    private var progress: (target: String, id: String, stage: String, expires: Date)?
    private var publication: (name: String, fingerprint: String, port: Int, enabled: Bool)?

    func invite(_ fingerprint: String, position: String) -> String {
        let id = UUID().uuidString
        invitation = (fingerprint, position, id, Date().addingTimeInterval(300))
        progress = nil
        publishChanges()
        return id
    }
    func cancelInvitation() { invitation = nil; progress = nil; publishChanges() }
    func reportProgress(_ stage: String, invitation id: String, target: String) {
        guard UUID(uuidString: id) != nil, normalizedFingerprint(target) != nil,
              ["reviewing", "approved"].contains(stage) else { return }
        progress = (target, id, stage, Date().addingTimeInterval(300))
        publishChanges()
    }
    func clearProgress(invitation id: String) {
        guard progress?.id == id else { return }
        progress = nil; publishChanges()
    }
    func resendInvitation() {
        guard invitation != nil else { return }
        advertisement?.stop(); advertisement = nil
        publishChanges()
    }
    private func publishChanges() {
        guard let publication else { return }
        update(name: publication.name, fingerprint: publication.fingerprint, port: publication.port, enabled: publication.enabled)
    }
    func invitationIsActive(_ id: String) -> Bool { invitation?.id == id && (invitation?.expires ?? .distantPast) > Date() }


    func update(name: String, fingerprint: String, port: Int, enabled: Bool) {
        publication = (name, fingerprint, port, enabled)
        if !started { NSLog("Lan Mouse: starting nearby discovery"); started = true; browser.delegate = self; browser.schedule(in: .main, forMode: .common); browser.searchForServices(ofType: "_lanmouse._udp.", inDomain: "local.") }
        guard enabled, !fingerprint.isEmpty else { advertisement?.stop(); advertisement = nil; return }
        if let invitation, invitation.expires < Date() { self.invitation = nil }
        if let progress, progress.expires < Date() { self.progress = nil }
        var records = ["fingerprint": Data(fingerprint.utf8)]
        if let progress {
            records["progressTarget"] = Data(progress.target.utf8)
            records["progressID"] = Data(progress.id.utf8)
            records["progressStage"] = Data(progress.stage.utf8)
        }
        if let invitation {
            records["target"] = Data(invitation.target.utf8)
            records["position"] = Data(invitation.position.utf8)
            records["invitation"] = Data(invitation.id.utf8)
        }
        // Give every transition a distinct record, including returning to idle.
        // Bonjour can otherwise suppress a return to a previously cached TXT value.
        var wireRecords = records
        wireRecords["update"] = Data(UUID().uuidString.utf8)
        let txt = NetService.data(fromTXTRecord: wireRecords)
        if let advertisement, ownFingerprint == fingerprint, advertisement.port == port {
            if records != lastRecords { advertisement.setTXTRecord(txt); lastRecords = records }
            return
        }
        advertisement?.stop(); ownFingerprint = fingerprint
        let service = NetService(domain: "local.", type: "_lanmouse._udp.", name: name, port: Int32(port))
        service.delegate = self
        service.schedule(in: .main, forMode: .common)
        service.setTXTRecord(txt); lastRecords = records
        NSLog("Lan Mouse: advertising nearby computer %@", name)
        service.publish(); advertisement = service
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        services.append(service); service.delegate = self; service.schedule(in: .main, forMode: .common); service.resolve(withTimeout: 10)
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didRemove service: NetService, moreComing: Bool) {
        services.first { $0 == service }?.stopMonitoring()
        services.removeAll { $0 == service }; nearby.removeAll { $0.id == service.name + service.domain }
    }
    func netServiceBrowser(_ browser: NetServiceBrowser, didNotSearch errorDict: [String: NSNumber]) {
        problem = "Nearby discovery is unavailable. Allow Local Network access in System Settings, or enter the other Mac’s address manually."
    }
    func netServiceDidPublish(_ sender: NetService) { NSLog("Lan Mouse: nearby advertisement ready") }
    func netService(_ sender: NetService, didNotPublish errorDict: [String: NSNumber]) {
        NSLog("Lan Mouse: nearby advertisement failed %@", errorDict)
        problem = "This Mac could not advertise itself. You can still pair using its hostname and fingerprint."
    }
    func netService(_ sender: NetService, didUpdateTXTRecord data: Data) {
        // The callback carries the new record; resolving again can reuse cached data.
        updateNearby(sender, data: data)
    }
    func netServiceDidResolveAddress(_ service: NetService) {
        updateNearby(service, data: service.txtRecordData() ?? Data())
        // Resolution and TXT monitoring are separate operations. Start the long-lived
        // monitor after resolution so later invitations reach an already open app.
        service.startMonitoring()
    }
    func updateNearby(_ service: NetService, data: Data) {
        let records = NetService.dictionary(fromTXTRecord: data)
        guard let raw = records["fingerprint"], let fp = normalizedFingerprint(String(decoding: raw, as: UTF8.self)),
              fp != ownFingerprint, let host = service.hostName else { return }
        let ips = (service.addresses ?? []).compactMap { data -> String? in
            data.withUnsafeBytes { bytes in
                guard let base = bytes.baseAddress else { return nil }
                let sa = base.assumingMemoryBound(to: sockaddr.self)
                var result = [CChar](repeating: 0, count: Int(NI_MAXHOST))
                guard getnameinfo(sa, socklen_t(data.count), &result, socklen_t(result.count), nil, 0, NI_NUMERICHOST) == 0 else { return nil }
                return String(cString: result)
            }
        }
        let value = { (key: String) in records[key].map { String(decoding: $0, as: UTF8.self) } }
        let peer = NearbyMac(id: service.name + service.domain, name: service.name, host: host, ips: ips, port: service.port, fingerprint: fp,
                             invitationTarget: value("target"), invitationPosition: value("position"), invitationID: value("invitation"),
                             progressTarget: value("progressTarget"), progressID: value("progressID"), progressStage: value("progressStage"))
        if let index = nearby.firstIndex(where: { $0.id == peer.id }) {
            if nearby[index] != peer { nearby[index] = peer }
        } else { nearby.append(peer) }
    }
}

struct PairRequest {
    let host: String
    let port: Int
    let position: String
    let fingerprint: String
}

