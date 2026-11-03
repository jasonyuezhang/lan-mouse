import AppKit
import IOKit

/// Secure Keyboard Entry hides keystrokes from every event tap, including the
/// daemon's capture, so typing cannot reach the other Mac while it is on.
enum SecureInput {
    struct Owner: Equatable {
        let pid: pid_t
        let name: String
    }

    /// The console session's secure-input PID, as recorded by WindowServer.
    /// It is the app in front when secure input was enabled, which is usually
    /// (not always) the app that enabled it.
    static func ownerPID(consoleUsers: [[String: Any]]) -> pid_t? {
        for session in consoleUsers where (session["kCGSSessionOnConsoleKey"] as? Bool) == true {
            if let pid = (session["kCGSSessionSecureInputPID"] as? NSNumber)?.int32Value, pid > 0 { return pid }
        }
        return nil
    }

    static func owner() -> Owner? {
        let root = IORegistryGetRootEntry(kIOMainPortDefault)
        defer { IOObjectRelease(root) }
        guard let users = IORegistryEntryCreateCFProperty(root, "IOConsoleUsers" as CFString, kCFAllocatorDefault, 0)?
                .takeRetainedValue() as? [[String: Any]],
              let pid = ownerPID(consoleUsers: users) else { return nil }
        return Owner(pid: pid, name: name(of: pid))
    }

    private static func name(of pid: pid_t) -> String {
        if let name = NSRunningApplication(processIdentifier: pid)?.localizedName { return name }
        var buffer = [CChar](repeating: 0, count: 256)
        guard proc_name(pid, &buffer, UInt32(buffer.count)) > 0 else { return "PID \(pid)" }
        return String(cString: buffer)
    }
}
