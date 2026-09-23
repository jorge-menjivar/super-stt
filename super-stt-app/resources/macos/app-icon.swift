// SPDX-License-Identifier: GPL-3.0-only
//
// Renders the macOS app icon from the app's SVG: the artwork on the rounded
// square every macOS app icon sits on, on the 1024-pixel grid Apple's icon
// template uses. `just bundle-macos` runs this, then scales the result into
// an .icns. The SVG stays the one source for the icon on every platform.
//
// Usage: swift app-icon.swift <icon.svg> <out.png>

import AppKit

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write("usage: app-icon.swift <icon.svg> <out.png>\n".data(using: .utf8)!)
    exit(2)
}
guard let artwork = NSImage(contentsOf: URL(fileURLWithPath: arguments[1])) else {
    FileHandle.standardError.write("cannot load \(arguments[1])\n".data(using: .utf8)!)
    exit(1)
}

// Apple's grid: an 824-point tile centred on a 1024-point canvas, with
// corners of radius 185.4. The margin is where the tile's shadow falls.
let canvas = 1024.0
let tile = 824.0
let cornerRadius = 185.4
let artworkSize = 620.0

let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(canvas), pixelsHigh: Int(canvas),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)

let inset = (canvas - tile) / 2
let shape = NSBezierPath(
    roundedRect: NSRect(x: inset, y: inset, width: tile, height: tile),
    xRadius: cornerRadius, yRadius: cornerRadius)

NSGraphicsContext.saveGraphicsState()
let shadow = NSShadow()
shadow.shadowColor = NSColor(white: 0, alpha: 0.3)
shadow.shadowBlurRadius = 20
shadow.shadowOffset = NSSize(width: 0, height: -10)
shadow.set()
NSColor.white.setFill()
shape.fill()
NSGraphicsContext.restoreGraphicsState()

// A warm light grey, so the artwork's black and olive both stand out.
NSGradient(
    starting: NSColor(srgbRed: 0.98, green: 0.98, blue: 0.95, alpha: 1),
    ending: NSColor(srgbRed: 0.85, green: 0.85, blue: 0.80, alpha: 1))!
    .draw(in: shape, angle: -90)

let origin = (canvas - artworkSize) / 2
artwork.draw(in: NSRect(x: origin, y: origin, width: artworkSize, height: artworkSize))

NSGraphicsContext.restoreGraphicsState()
try bitmap.representation(using: .png, properties: [:])!
    .write(to: URL(fileURLWithPath: arguments[2]))
