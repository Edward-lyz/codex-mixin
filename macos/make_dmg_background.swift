import AppKit
import Foundation

guard CommandLine.arguments.count == 2 else {
    fatalError("usage: make_dmg_background.swift <output-png>")
}

let outputURL = URL(fileURLWithPath: CommandLine.arguments[1])
let size = NSSize(width: 660, height: 420)
let image = NSImage(size: size)

image.lockFocus()

let bounds = NSRect(origin: .zero, size: size)
NSColor(calibratedWhite: 0.97, alpha: 1).setFill()
bounds.fill()

let background = NSBezierPath(roundedRect: bounds.insetBy(dx: 18, dy: 18), xRadius: 28, yRadius: 28)
NSGradient(colors: [
    NSColor(calibratedRed: 0.93, green: 0.96, blue: 1.0, alpha: 1),
    NSColor(calibratedRed: 0.98, green: 0.97, blue: 1.0, alpha: 1),
])?.draw(in: background, angle: 35)

let ribbon = NSBezierPath()
ribbon.move(to: NSPoint(x: 70, y: 96))
ribbon.curve(to: NSPoint(x: 580, y: 330), controlPoint1: NSPoint(x: 210, y: 365), controlPoint2: NSPoint(x: 440, y: 40))
ribbon.lineWidth = 42
ribbon.lineCapStyle = .round
NSColor(calibratedRed: 0.18, green: 0.50, blue: 1.0, alpha: 0.18).setStroke()
ribbon.stroke()

let ribbon2 = NSBezierPath()
ribbon2.move(to: NSPoint(x: 92, y: 320))
ribbon2.curve(to: NSPoint(x: 570, y: 90), controlPoint1: NSPoint(x: 250, y: 120), controlPoint2: NSPoint(x: 395, y: 420))
ribbon2.lineWidth = 36
ribbon2.lineCapStyle = .round
NSColor(calibratedRed: 0.58, green: 0.28, blue: 1.0, alpha: 0.16).setStroke()
ribbon2.stroke()

let title = "Codex Mixin"
let subtitle = "Drag the app to Applications. CLI is in bin/codex-mixin."
let titleAttributes: [NSAttributedString.Key: Any] = [
    .font: NSFont.boldSystemFont(ofSize: 30),
    .foregroundColor: NSColor(calibratedWhite: 0.16, alpha: 1),
]
let subtitleAttributes: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 14),
    .foregroundColor: NSColor(calibratedWhite: 0.42, alpha: 1),
]
title.draw(at: NSPoint(x: 44, y: 354), withAttributes: titleAttributes)
subtitle.draw(at: NSPoint(x: 45, y: 329), withAttributes: subtitleAttributes)

let arrow = NSBezierPath()
arrow.lineWidth = 5
arrow.lineCapStyle = .round
arrow.lineJoinStyle = .round
arrow.move(to: NSPoint(x: 270, y: 205))
arrow.line(to: NSPoint(x: 390, y: 205))
arrow.line(to: NSPoint(x: 372, y: 221))
arrow.move(to: NSPoint(x: 390, y: 205))
arrow.line(to: NSPoint(x: 372, y: 189))
NSColor(calibratedRed: 0.27, green: 0.45, blue: 0.95, alpha: 0.72).setStroke()
arrow.stroke()

image.unlockFocus()

guard
    let tiff = image.tiffRepresentation,
    let bitmap = NSBitmapImageRep(data: tiff),
    let png = bitmap.representation(using: .png, properties: [:])
else {
    fatalError("failed to render dmg background")
}

try FileManager.default.createDirectory(at: outputURL.deletingLastPathComponent(), withIntermediateDirectories: true)
try png.write(to: outputURL)
