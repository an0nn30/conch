import AppKit
// scan.swift <image> <axis:col|row> <fixedCoord> [start] [end]
// prints runs of identical color along the scan line: color, start, length
let a = CommandLine.arguments
guard a.count > 3, let img = NSImage(contentsOfFile: a[1]),
      let tiff = img.tiffRepresentation, let bmp = NSBitmapImageRep(data: tiff) else { exit(1) }
let axis = a[2], fixed = Int(a[3])!
let start = a.count > 4 ? Int(a[4])! : 0
let end = a.count > 5 ? Int(a[5])! : (axis == "col" ? bmp.pixelsHigh : bmp.pixelsWide)
func hex(_ i: Int) -> String {
    let c = axis == "col" ? bmp.colorAt(x: fixed, y: i) : bmp.colorAt(x: i, y: fixed)
    guard let c = c else { return "?" }
    return String(format: "#%02X%02X%02X", Int(c.redComponent*255+0.5), Int(c.greenComponent*255+0.5), Int(c.blueComponent*255+0.5))
}
var runStart = start, prev = hex(start)
for i in (start+1)..<end {
    let h = hex(i)
    if h != prev {
        let len = i - runStart
        if len >= 2 { print("\(prev)\t\(runStart)\t len=\(len)") }
        runStart = i; prev = h
    }
}
print("\(prev)\t\(runStart)\t len=\(end - runStart)")
