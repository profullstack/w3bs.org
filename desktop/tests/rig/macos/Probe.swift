// Independent read-back for the macOS end-to-end rig (does not use the code under test).
//   probe mouse     pointer position, Quartz points: "x y"
//   probe windows   on-screen window list (layer, owner, pid, number, bounds, name)
//   probe hid       seconds since the last hardware (HID system state) input event
//   probe tcc       what this process is allowed (Accessibility, post events, screen capture)
//   probe hotkey    post Control+Option+Command+Escape (the emergency-stop chord)
import AppKit
import ApplicationServices
import Carbon
import CoreGraphics

let cmd = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ""
switch cmd {
case "mouse":
    let p = CGEvent(source: nil)!.location
    print("\(p.x) \(p.y)")
case "windows":
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as! [[String: Any]]
    for w in list {
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        print("layer=\(w[kCGWindowLayer as String] ?? "") owner=\(w[kCGWindowOwnerName as String] ?? "") pid=\(w[kCGWindowOwnerPID as String] ?? "") id=\(w[kCGWindowNumber as String] ?? "") alpha=\(w[kCGWindowAlpha as String] ?? "") bounds=\(b["X"] ?? ""),\(b["Y"] ?? ""),\(b["Width"] ?? ""),\(b["Height"] ?? "") name=\(w[kCGWindowName as String] ?? "")")
    }
case "hid":
    print(CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: CGEventType(rawValue: ~0)!))
case "tcc":
    print("accessibility=\(AXIsProcessTrusted()) post_events=\(CGPreflightPostEventAccess()) screen_capture=\(CGPreflightScreenCaptureAccess()) secure_input=\(IsSecureEventInputEnabled())")
case "hotkey":
    let src = CGEventSource(stateID: .hidSystemState)
    let flags: CGEventFlags = [.maskControl, .maskAlternate, .maskCommand]
    for down in [true, false] {
        let e = CGEvent(keyboardEventSource: src, virtualKey: 0x35, keyDown: down)!
        e.flags = flags
        e.post(tap: .cghidEventTap)
        usleep(50_000)
    }
default:
    FileHandle.standardError.write("usage: probe mouse|windows|hid|tcc|hotkey\n".data(using: .utf8)!)
    exit(2)
}
