import Carbon
import Foundation

@main struct SecureInputTests {
    static func main() {
        // Only the console session's positive PID counts.
        assert(SecureInput.ownerPID(consoleUsers: []) == nil)
        assert(SecureInput.ownerPID(consoleUsers: [["kCGSSessionOnConsoleKey": true]]) == nil)
        assert(SecureInput.ownerPID(consoleUsers: [["kCGSSessionOnConsoleKey": true, "kCGSSessionSecureInputPID": 0]]) == nil)
        assert(SecureInput.ownerPID(consoleUsers: [
            ["kCGSSessionOnConsoleKey": false, "kCGSSessionSecureInputPID": 11],
            ["kCGSSessionOnConsoleKey": true, "kCGSSessionSecureInputPID": 42],
        ]) == 42)
        print("Secure input parsing checks passed")

        // Optional live check: briefly hold secure input and confirm detection.
        if CommandLine.arguments.contains("--live") {
            guard SecureInput.owner() == nil else { fatalError("Another app already holds secure input") }
            EnableSecureEventInput()
            defer { DisableSecureEventInput() }
            // WindowServer records it asynchronously, against the responsible
            // app (the terminal when run from a shell), not this process.
            Thread.sleep(forTimeInterval: 0.5)
            guard let owner = SecureInput.owner() else { fatalError("Secure input owner not detected") }
            print("Live detection passed: \(owner.name) (\(owner.pid))")
        }
    }
}
