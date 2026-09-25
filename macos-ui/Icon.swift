import AppKit
import Foundation
let destination = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
for size in [16,32,128,256,512] {
    for scale in [1,2] {
        let pixels = size * scale
        let image = NSImage(size: NSSize(width: pixels, height: pixels))
        image.lockFocus()
        let side = CGFloat(pixels)
        let rect = NSRect(x: side * 0.04, y: side * 0.04, width: side * 0.92, height: side * 0.92)
        let path = NSBezierPath(roundedRect: rect, xRadius: side * 0.22, yRadius: side * 0.22)
        NSGradient(starting: NSColor.systemBlue, ending: NSColor.systemIndigo)!.draw(in: path, angle: -70)
        if let symbol = NSImage(systemSymbolName: "computermouse.fill", accessibilityDescription: nil)?.withSymbolConfiguration(.init(paletteColors: [.white])) {
            symbol.draw(in: NSRect(x: side * 0.29, y: side * 0.20, width: side * 0.42, height: side * 0.60))
        }
        image.unlockFocus()
        let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
        try bitmap.representation(using: .png, properties: [:])!.write(to: destination.appendingPathComponent("icon_\(size)x\(size)\(scale == 2 ? "@2x" : "").png"))
    }
}
