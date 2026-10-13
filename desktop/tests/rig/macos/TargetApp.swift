// Deterministic target for the macOS end-to-end rig: two windows ("ADL Target" with a text
// field, "ADL Other" with another), each writing what it receives to files the rig reads.
//   TargetApp <out-dir>
// Files: ready, geometry.json (Quartz top-left coordinates), target.txt, other.txt, key.txt,
// clicks.txt (click count of the last mouse-down in the target window).
import AppKit

let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/adl-target")
try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
func write(_ name: String, _ text: String) {
    try? text.write(to: out.appendingPathComponent(name), atomically: true, encoding: .utf8)
}

final class ClickView: NSView {
    override func mouseDown(with event: NSEvent) {
        write("clicks.txt", "\(event.clickCount)")
        super.mouseDown(with: event)
    }
}

final class Delegate: NSObject, NSApplicationDelegate, NSTextFieldDelegate, NSWindowDelegate {
    var windows: [NSWindow] = []
    var fields: [NSTextField] = []

    func make(_ title: String, x: CGFloat, top: CGFloat) -> (NSWindow, NSTextField) {
        let screen = NSScreen.screens[0].frame
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 400, height: 260), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        w.title = title
        w.isReleasedWhenClosed = false
        w.contentView = ClickView(frame: NSRect(x: 0, y: 0, width: 400, height: 260))
        w.setFrameTopLeftPoint(NSPoint(x: x, y: screen.height - top))
        let f = NSTextField(frame: NSRect(x: 20, y: 200, width: 360, height: 28))
        f.identifier = NSUserInterfaceItemIdentifier(title == "ADL Target" ? "adl-target-field" : "adl-other-field")
        f.setAccessibilityLabel(title + " field")
        f.delegate = self
        w.contentView!.addSubview(f)
        w.delegate = self
        return (w, f)
    }

    /// Quartz (top-left origin) rectangle of a view, for the rig.
    func quartz(_ v: NSView) -> [String: CGFloat] {
        let r = v.window!.convertToScreen(v.convert(v.bounds, to: nil))
        let h = NSScreen.screens[0].frame.height
        return ["x": r.origin.x, "y": h - r.origin.y - r.size.height, "width": r.size.width, "height": r.size.height]
    }

    func applicationDidFinishLaunching(_ n: Notification) {
        let (t, tf) = make("ADL Target", x: 60, top: 120)
        let (o, of) = make("ADL Other", x: 520, top: 120)
        windows = [t, o]
        fields = [tf, of]
        o.orderFront(nil)
        t.makeKeyAndOrderFront(nil)
        t.makeFirstResponder(tf)
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            let geo: [String: Any] = [
                "target_window": self.quartz(t.contentView!), "target_field": self.quartz(tf),
                "other_window": self.quartz(o.contentView!), "other_field": self.quartz(of),
                "pid": ProcessInfo.processInfo.processIdentifier,
            ]
            let data = try! JSONSerialization.data(withJSONObject: geo, options: [.sortedKeys])
            write("geometry.json", String(data: data, encoding: .utf8)!)
            write("ready", "1")
        }
    }

    func controlTextDidChange(_ obj: Notification) {
        write("target.txt", fields[0].stringValue)
        write("other.txt", fields[1].stringValue)
    }

    func windowDidBecomeKey(_ n: Notification) {
        write("key.txt", (n.object as? NSWindow)?.title ?? "")
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
// Key equivalents (Command+A, ...) come from the main menu, as in any real app.
let mainMenu = NSMenu()
let appItem = NSMenuItem()
appItem.submenu = NSMenu()
appItem.submenu!.addItem(withTitle: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
let editItem = NSMenuItem()
editItem.submenu = NSMenu(title: "Edit")
editItem.submenu!.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
editItem.submenu!.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
editItem.submenu!.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
editItem.submenu!.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
mainMenu.addItem(appItem)
mainMenu.addItem(editItem)
app.mainMenu = mainMenu
let delegate = Delegate()
app.delegate = delegate
app.run()
