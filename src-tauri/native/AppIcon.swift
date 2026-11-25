import AppKit

/// Writes the icon of the app at `path` to standard output as one 128 x 128 PNG.
/// The caller passes a path from its own list of installed apps.
func writeAppIcon(_ path: String?) -> Int32 {
    guard let path = path, path.hasPrefix("/"), path.hasSuffix(".app") else { return EXIT_FAILURE }
    let image = NSWorkspace.shared.icon(forFile: path)
    let size = 128
    guard let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0
    ), let context = NSGraphicsContext(bitmapImageRep: rep) else { return EXIT_FAILURE }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    image.draw(in: NSRect(x: 0, y: 0, width: size, height: size))
    NSGraphicsContext.restoreGraphicsState()
    guard let data = rep.representation(using: .png, properties: [:]) else { return EXIT_FAILURE }
    FileHandle.standardOutput.write(data)
    return EXIT_SUCCESS
}
