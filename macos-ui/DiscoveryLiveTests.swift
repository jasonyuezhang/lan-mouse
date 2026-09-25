import Foundation
@main struct DiscoveryLiveTests {
    @MainActor static func wait(_ reason: String, _ check: () -> Bool) async {
        let deadline = Date().addingTimeInterval(15)
        while !check(), Date() < deadline { try? await Task.sleep(nanoseconds: 200_000_000) }
        guard check() else { fatalError(reason) }
    }
    @MainActor static func main() async {
        let a = Discovery(), b = Discovery()
        let af = normalizedFingerprint(String(repeating: "12", count: 32))!
        let bf = normalizedFingerprint(String(repeating: "34", count: 32))!
        let suffix = String(UUID().uuidString.prefix(6))
        a.update(name: "Lan Mouse test A " + suffix, fingerprint: af, port: 42420, enabled: true)
        b.update(name: "Lan Mouse test B " + suffix, fingerprint: bf, port: 42421, enabled: true)
        await wait("initial discovery") { b.nearby.contains { $0.fingerprint == af } && a.nearby.contains { $0.fingerprint == bf } }
        let id = a.invite(bf, position: "left")
        await wait("invitation after initial discovery") { b.nearby.contains { $0.fingerprint == af && $0.invitationID == id } }
        b.reportProgress("reviewing", invitation: id, target: af)
        await wait("review acknowledgement") { a.nearby.contains { $0.fingerprint == bf && $0.progress(for: id, target: af) == "reviewing" } }
        b.reportProgress("approved", invitation: id, target: af)
        await wait("approval update") { a.nearby.contains { $0.fingerprint == bf && $0.progress(for: id, target: af) == "approved" } }
        b.clearProgress(invitation: UUID().uuidString)
        assert(a.nearby.contains { $0.fingerprint == bf && $0.progress(for: id, target: af) == "approved" })
        b.clearProgress(invitation: id)
        await wait("clear review progress after dismissal") { a.nearby.contains { $0.fingerprint == bf && $0.progressID == nil } }
        a.cancelInvitation()
        await wait("cancel update") { b.nearby.contains { $0.fingerprint == af && $0.invitationID == nil } }
        a.update(name: "Lan Mouse test A " + suffix, fingerprint: af, port: 42420, enabled: false)
        b.update(name: "Lan Mouse test B " + suffix, fingerprint: bf, port: 42421, enabled: false)
        print("PASS: live Bonjour discovery, later invitation, review/approval and cancellation without reopening")
    }
}
