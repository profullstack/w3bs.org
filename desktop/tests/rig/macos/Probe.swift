// Independent read-back for the macOS end-to-end rig (does not use the code under test).
//   probe mouse     pointer position, Quartz points: "x y"
//   probe windows   on-screen window list (layer, owner, pid, number, bounds, name)
//   probe hid       seconds since the last hardware (HID system state) input event
//   probe tcc       what this process is allowed (Accessibility, post events, screen capture)
//   probe hotkey    post Control+Option+Command+Escape (the emergency-stop chord)
//   probe click X Y a left click at Quartz point (X, Y)
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
    // Modifier presses as flags-changed events, then Escape, then the releases, the way a
    // keyboard delivers the chord.
    let src = CGEventSource(stateID: .hidSystemState)
    func post(_ code: CGKeyCode, _ down: Bool, _ flags: CGEventFlags, modifier: Bool) {
        let e = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down)!
        if modifier { e.type = .flagsChanged }
        e.flags = flags
        e.post(tap: .cghidEventTap)
        usleep(40_000)
    }
    let mods: [(CGKeyCode, CGEventFlags)] = [(0x3B, .maskControl), (0x3A, .maskAlternate), (0x37, .maskCommand)]
    var flags: CGEventFlags = []
    for (code, flag) in mods { flags.insert(flag); post(code, true, flags, modifier: true) }
    post(0x35, true, flags, modifier: false)
    post(0x35, false, flags, modifier: false)
    for (code, flag) in mods.reversed() { flags.remove(flag); post(code, false, flags, modifier: true) }
case "click":
    // probe click X Y: a real left click at Quartz point (X, Y).
    let x = Double(CommandLine.arguments[2])!, y = Double(CommandLine.arguments[3])!
    let src = CGEventSource(stateID: .hidSystemState)
    for t in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
        CGEvent(mouseEventSource: src, mouseType: t, mouseCursorPosition: CGPoint(x: x, y: y), mouseButton: .left)!.post(tap: .cghidEventTap)
        usleep(60_000)
    }
default:
    FileHandle.standardError.write("usage: probe mouse|windows|hid|tcc|hotkey|click X Y\n".data(using: .utf8)!)
    exit(2)
}
