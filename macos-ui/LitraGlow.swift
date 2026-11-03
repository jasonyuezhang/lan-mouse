import Foundation
import Combine
import IOKit.hid

enum LitraGlow {
    enum Failure: LocalizedError {
        case multipleDevices, usb(IOReturn), noResponse

        var errorDescription: String? {
            switch self {
            case .multipleDevices: return "Connect one Litra Glow to control its power."
            case .usb(let code): return "Couldn’t access Litra Glow (USB error \(code)). Reconnect its USB cable and retry."
            case .noResponse: return "Litra Glow didn’t report its power state. Reconnect its USB cable and retry."
            }
        }
    }

    // Glow's HID++ lighting feature is 0x04. The low nibble identifies our
    // requests, so unrelated G HUB replies cannot be mistaken for our own.
    // Protocol reference: https://github.com/timrogers/litra/blob/main/src/driver.ts
    static func report(power: Bool?) -> [UInt8] {
        [0x11, 0xff, 0x04, power == nil ? 0x0b : 0x1b, power == true ? 1 : 0]
            + Array(repeating: 0, count: 15)
    }

    static func powerState(_ report: [UInt8]) -> Bool? {
        guard report.count == 20, report.prefix(4).elementsEqual([0x11, 0xff, 0x04, 0x0b]), report[4] <= 1 else { return nil }
        return report[4] == 1
    }

    private final class Reply {
        var power: Bool?
    }

    // Run on a worker: IOHIDSetReport is synchronous. Open without seizing the
    // device, allowing its physical controls and Logitech's app to coexist.
    static func access(power: Bool? = nil) throws -> Bool? {
        let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
        IOHIDManagerSetDeviceMatching(manager, [
            kIOHIDVendorIDKey: 0x046d,
            kIOHIDProductIDKey: 0xc900,
            kIOHIDDeviceUsagePageKey: 0xff43
        ] as CFDictionary)
        let opened = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        guard opened == kIOReturnSuccess else { throw Failure.usb(opened) }
        defer { IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone)) }
        let devices = IOHIDManagerCopyDevices(manager) as? Set<IOHIDDevice> ?? []
        guard devices.count <= 1 else { throw Failure.multipleDevices }
        guard let device = devices.first else { return nil }

        let reply = Reply()
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 64)
        let runLoop = CFRunLoopGetCurrent()!
        IOHIDDeviceRegisterInputReportCallback(device, buffer, 64, { context, result, _, _, _, bytes, length in
            guard result == kIOReturnSuccess, let context else { return }
            let reply = Unmanaged<Reply>.fromOpaque(context).takeUnretainedValue()
            if let state = LitraGlow.powerState(Array(UnsafeBufferPointer(start: bytes, count: length))) {
                reply.power = state
            }
        }, Unmanaged.passUnretained(reply).toOpaque())
        IOHIDDeviceScheduleWithRunLoop(device, runLoop, CFRunLoopMode.defaultMode.rawValue)
        defer {
            IOHIDDeviceUnscheduleFromRunLoop(device, runLoop, CFRunLoopMode.defaultMode.rawValue)
            IOHIDDeviceRegisterInputReportCallback(device, buffer, 64, nil, nil)
            buffer.deallocate()
        }

        func send(_ power: Bool?) throws {
            let bytes = report(power: power)
            let result = IOHIDDeviceSetReport(device, kIOHIDReportTypeOutput, 0x11, bytes, bytes.count)
            guard result == kIOReturnSuccess else { throw Failure.usb(result) }
        }
        if let power { try send(power) }
        try send(nil)
        let deadline = ProcessInfo.processInfo.systemUptime + 1
        while reply.power == nil && ProcessInfo.processInfo.systemUptime < deadline {
            CFRunLoopRunInMode(.defaultMode, 0.05, true)
        }
        guard let state = reply.power else { throw Failure.noResponse }
        return state
    }
}

@MainActor final class LitraGlowController: ObservableObject {
    @Published private(set) var power: Bool?
    @Published private(set) var busy = false
    @Published private(set) var error: String?

    func update(power requested: Bool? = nil) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            power = try await Task.detached(priority: .userInitiated) { try LitraGlow.access(power: requested) }.value
            error = nil
        } catch {
            power = nil
            self.error = error.localizedDescription
        }
    }
}
