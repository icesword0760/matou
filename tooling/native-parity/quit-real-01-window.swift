import AppKit
import CoreGraphics
import Darwin

let title = "Matou Native"

func now() -> UInt64 { clock_gettime_nsec_np(CLOCK_REALTIME) }

func record(_ event: String, _ fields: [String: Any] = [:]) {
    var row = fields
    row["event"] = event
    row["unix_ns"] = now()
    let data = try! JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
    FileHandle.standardOutput.write(data + Data([10]))
}

func fail(_ message: String) -> Never {
    record("refused", ["reason": message])
    exit(1)
}

func windows() -> [[String: Any]] {
    let all = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], .zero)
        as? [[String: Any]] ?? []
    return all.compactMap {
        guard $0[kCGWindowLayer as String] as? Int == 0,
              ($0[kCGWindowAlpha as String] as? Double ?? 0) > 0,
              let bounds = $0[kCGWindowBounds as String] as? [String: Any],
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary),
              rect.width > 0, rect.height > 0 else { return nil }
        return ["window": $0[kCGWindowNumber as String] ?? NSNull(),
                "owner_pid": $0[kCGWindowOwnerPID as String] ?? NSNull(),
                "title": $0[kCGWindowName as String] ?? "", "bounds": bounds]
    }
}

let args = CommandLine.arguments
guard args.count == 4, let pid = Int32(args[1]), pid > 1,
      ["quit-real-01", "quit-fix-01"].contains(where: {
          args[2].hasSuffix("/evidence/\($0)/MatouNative.app")
      }) else {
    fail("The expected isolated app path did not match")
}
let evidence = URL(fileURLWithPath: args[2]).deletingLastPathComponent()
let bundle = "com.matou.\(evidence.lastPathComponent)"
guard URL(fileURLWithPath: args[3]).deletingLastPathComponent().path == evidence.path,
      let app = NSRunningApplication(processIdentifier: pid),
      app.bundleIdentifier == bundle, app.bundleURL?.path == args[2] else {
    fail("The expected isolated PID and bundle did not match")
}
guard CGPreflightPostEventAccess(), CGPreflightScreenCaptureAccess() else {
    fail("Desktop event posting or screen capture permission is unavailable")
}

func checkedWindow(_ windowID: Int? = nil) -> [String: Any]? {
    guard let active = NSWorkspace.shared.frontmostApplication,
          active.processIdentifier == pid, active.bundleIdentifier == bundle,
          active.bundleURL?.path == args[2], !app.isTerminated else { return nil }
    guard let top = windows().first, top["owner_pid"] as? Int32 == pid,
          top["title"] as? String == title, let id = top["window"] as? Int,
          windowID == nil || windowID == id else { return nil }
    return top
}

func capture(_ id: Int, _ name: String) -> Process? {
    let image = Process()
    image.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    image.arguments = ["-x", "-l", String(id), "\(args[3])/\(name).png"]
    do {
        try image.run()
        return image
    } catch {
        record("screenshot_error", ["name": name, "error": String(describing: error)])
        return nil
    }
}

func captureResult(_ image: Process, _ name: String) {
    image.waitUntilExit()
    record("screenshot_result", ["name": name, "status": image.terminationStatus])
}

let activationDeadline = DispatchTime.now().uptimeNanoseconds + 4_000_000_000
var target = checkedWindow()
while target == nil && DispatchTime.now().uptimeNanoseconds < activationDeadline {
    _ = app.activate(options: [.activateIgnoringOtherApps])
    usleep(100_000)
    target = checkedWindow()
}
guard let verified = target, let id = verified["window"] as? Int else {
    fail("The isolated PID never owned the top normal CG window with the exact title")
}
if let before = capture(id, "before") { captureResult(before, "before") }

guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 12, keyDown: true),
      let up = CGEvent(keyboardEventSource: nil, virtualKey: 12, keyDown: false) else {
    fail("Could not construct Cmd+Q")
}
down.flags = .maskCommand
up.flags = .maskCommand
guard let checked = checkedWindow(id) else { fail("Foreground identity changed before Cmd+Q") }
record("verified_target", ["pid": pid, "bundle": bundle, "app": args[2], "top_window": checked])
guard checkedWindow(id) != nil else { fail("Foreground identity changed at Cmd+Q") }
let posted = now()
down.post(tap: .cghidEventTap)
up.post(tap: .cghidEventTap)
record("cmd_q_posted", ["keydown_unix_ns": posted, "pid": pid, "window": id,
                        "source": "CG HID event tap, ANSI Q with Command"])

let windowStart = DispatchTime.now().uptimeNanoseconds
var lastVisible = true
var lastVisibleTime = posted
var during: Process?
var attemptedDuring = false
let backups = URL(fileURLWithPath: args[3]).appendingPathComponent("data/backups")
func backupComplete() -> Bool {
    guard let files = try? FileManager.default.contentsOfDirectory(atPath: backups.path) else { return false }
    return files.contains { name in
        name.contains("-clean-exit-") && name.hasSuffix(".sqlite") &&
            files.contains(String(name.dropLast(".sqlite".count)) + ".json")
    }
}
while DispatchTime.now().uptimeNanoseconds - windowStart < 15_300_000_000 {
    // Read the file pair before sampling the window. A complete pair at the
    // first off-screen sample brackets release even if polling sees a tie.
    let complete = backupComplete()
    let visible = windows().contains { $0["window"] as? Int == id && $0["owner_pid"] as? Int32 == pid }
    let sampled = now()
    if visible { lastVisibleTime = sampled }
    if visible != lastVisible {
        record(visible ? "window_back_on_screen" : "window_left_screen",
               ["sample_unix_ns": sampled, "last_visible_unix_ns": lastVisibleTime, "window": id,
                "backup_complete_before_sample": complete])
        lastVisible = visible
    }
    if visible && !attemptedDuring && sampled > posted + 350_000_000 {
        attemptedDuring = true
        during = capture(id, "during-wait")
    }
    if kill(pid, 0) == -1 && errno == ESRCH {
        record("pid_absent_from_window_probe", ["pid": pid, "window_visible": visible])
        break
    }
    usleep(4_000)
}
if let during { captureResult(during, "during-wait") }
