import AppKit
let args = CommandLine.arguments
guard args.count > 2, let img = NSImage(contentsOfFile: args[1]),
      let tiff = img.tiffRepresentation, let bmp = NSBitmapImageRep(data: tiff) else { exit(1) }
for pair in args[2...] {
    let p = pair.split(separator: ",").compactMap { Int($0) }
    guard p.count == 2, let c = bmp.colorAt(x: p[0], y: p[1]) else { continue }
    let r = Int(c.redComponent*255+0.5), g = Int(c.greenComponent*255+0.5), b = Int(c.blueComponent*255+0.5)
    print(String(format: "%@ -> #%02X%02X%02X", pair, r, g, b))
}
