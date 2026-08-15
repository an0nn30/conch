import CoreGraphics
import Foundation
let opts: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] else { exit(1) }
for w in list {
    let owner = (w[kCGWindowOwnerName as String] as? String) ?? "?"
    let name = (w[kCGWindowName as String] as? String) ?? ""
    let num = (w[kCGWindowNumber as String] as? Int) ?? -1
    let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
    let wd = (b["Width"] as? Double) ?? 0, ht = (b["Height"] as? Double) ?? 0
    if wd < 300 || ht < 200 { continue }
    print("\(num)\t\(Int(wd))x\(Int(ht))\t\(owner)\t\(name)")
}
