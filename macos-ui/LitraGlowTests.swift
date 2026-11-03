import Foundation

@main struct LitraGlowTests {
    static func main() throws {
        // Reject unrelated replies, malformed packets, and unknown power values.
        var reply: [UInt8] = [0x11, 0xff, 0x04, 0x0b, 1] + Array(repeating: 0, count: 15)
        assert(LitraGlow.powerState(reply) == true)
        reply[4] = 0
        assert(LitraGlow.powerState(reply) == false)
        reply[4] = 2
        assert(LitraGlow.powerState(reply) == nil)
        reply[4] = 1
        reply[3] = 0x1b // A write acknowledgement isn't a power-state reply.
        assert(LitraGlow.powerState(reply) == nil)
        reply[3] = 0x01 // A different app's request.
        assert(LitraGlow.powerState(reply) == nil)
        assert(LitraGlow.powerState([]) == nil)
        assert(LitraGlow.powerState([0x11, 0xff, 0x04, 0x0b]) == nil)
        print("Litra Glow protocol checks passed")

        // Optional hardware checks; ordinary tests never change a connected light.
        if CommandLine.arguments.contains("--read") || CommandLine.arguments.contains("--cycle") {
            guard let original = try LitraGlow.access() else { fatalError("Connect a Litra Glow first") }
            print("Litra Glow power: \(original ? "on" : "off")")
            if CommandLine.arguments.contains("--cycle") {
                defer { _ = try? LitraGlow.access(power: original) }
                guard try LitraGlow.access(power: !original) == !original else { throw LitraGlow.Failure.noResponse }
                guard try LitraGlow.access(power: original) == original else { throw LitraGlow.Failure.noResponse }
                print("Hardware on/off cycle passed; original power restored")
            }
        }
    }
}
