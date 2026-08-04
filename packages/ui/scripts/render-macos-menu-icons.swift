#!/usr/bin/env swift

import AppKit
import Foundation

struct MenuSymbol {
  let filename: String
  let name: String
}

let symbols = [
  MenuSymbol(filename: "wifi.png", name: "wifi"),
  MenuSymbol(filename: "battery-charging.png", name: "battery.100percent.bolt"),
  MenuSymbol(filename: "search.png", name: "magnifyingglass"),
]

let outputPath = CommandLine.arguments.dropFirst().first
  ?? "src/assets/macos-menu-bar"
let outputDirectory = URL(
  fileURLWithPath: outputPath,
  isDirectory: true
)

try FileManager.default.createDirectory(
  at: outputDirectory,
  withIntermediateDirectories: true
)

let maximumSymbolSize: CGFloat = 52
let symbolPadding: CGFloat = 5
let pointConfiguration = NSImage.SymbolConfiguration(
  pointSize: 30,
  weight: .regular
)
let whiteConfiguration = NSImage.SymbolConfiguration(paletteColors: [.white])
let configuration = pointConfiguration.applying(whiteConfiguration)

for symbol in symbols {
  guard let systemImage = NSImage(
    systemSymbolName: symbol.name,
    accessibilityDescription: nil
  ), let image = systemImage.withSymbolConfiguration(configuration) else {
    fatalError("Missing macOS system symbol: \(symbol.name)")
  }

  let intrinsicSize = image.size
  let scale = min(
    maximumSymbolSize / intrinsicSize.width,
    maximumSymbolSize / intrinsicSize.height
  )
  let drawSize = NSSize(
    width: intrinsicSize.width * scale,
    height: intrinsicSize.height * scale
  )
  let canvasSize = NSSize(
    width: ceil(drawSize.width + symbolPadding * 2),
    height: ceil(drawSize.height + symbolPadding * 2)
  )

  guard let bitmap = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: Int(canvasSize.width),
    pixelsHigh: Int(canvasSize.height),
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
  ) else {
    fatalError("Could not create bitmap for \(symbol.name)")
  }

  bitmap.size = canvasSize
  let context = NSGraphicsContext(bitmapImageRep: bitmap)
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = context
  NSColor.clear.setFill()
  NSRect(origin: .zero, size: canvasSize).fill()

  let drawRect = NSRect(
    x: (canvasSize.width - drawSize.width) / 2,
    y: (canvasSize.height - drawSize.height) / 2,
    width: drawSize.width,
    height: drawSize.height
  )
  image.draw(in: drawRect)
  context?.flushGraphics()
  NSGraphicsContext.restoreGraphicsState()

  guard let png = bitmap.representation(using: .png, properties: [:]) else {
    fatalError("Could not encode \(symbol.name)")
  }
  try png.write(to: outputDirectory.appendingPathComponent(symbol.filename))
}

_ = NSApplication.shared
let cursorImage = NSCursor.arrow.image
guard let cursorRepresentation = cursorImage.representations
  .compactMap({ $0 as? NSBitmapImageRep })
  .first(where: { $0.pixelsWide == 34 && $0.pixelsHigh == 46 }),
  let cursorPng = cursorRepresentation.representation(
    using: .png,
    properties: [:]
  )
else {
  fatalError("Could not read the native macOS arrow cursor")
}
try cursorPng.write(
  to: outputDirectory.appendingPathComponent("cursor-arrow.png")
)
