import AppKit
import CoreGraphics
import CryptoKit
import Darwin
import ImageIO
import UniformTypeIdentifiers

func emit(_ event: String, _ detail: [String: Any] = [:]) {
    var line = detail
    line["event"] = event
    line["unix_ns"] = clock_gettime_nsec_np(CLOCK_REALTIME)
    let bytes = try! JSONSerialization.data(withJSONObject: line, options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes + Data([10]))
}

func fail(_ reason: String) -> Never {
    emit("refused", ["reason": reason])
    exit(1)
}

struct Window {
    var id: CGWindowID
    var pid: Int32
    var title: String
    var rect: CGRect
    var layer: Int
    var alpha: Double
}

func windows() -> [Window] {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], 0)
        as? [[String: Any]] ?? []
    return list.compactMap { row in
        guard let id = row[kCGWindowNumber as String] as? CGWindowID,
              let pid = row[kCGWindowOwnerPID as String] as? Int32,
              let bounds = row[kCGWindowBounds as String] as? [String: CGFloat],
              let x = bounds["X"], let y = bounds["Y"], let w = bounds["Width"], let h = bounds["Height"],
              let layer = row[kCGWindowLayer as String] as? Int else { return nil }
        return Window(id: id, pid: pid, title: row[kCGWindowName as String] as? String ?? "",
                      rect: CGRect(x: x, y: y, width: w, height: h), layer: layer,
                      alpha: row[kCGWindowAlpha as String] as? Double ?? 0)
    }
}

func identity(_ window: Window) -> [String: Any] {
    ["window_id": window.id, "owner_pid": window.pid, "title": window.title,
     "frontmost_pid": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1,
     "bounds_pt": [window.rect.minX, window.rect.minY, window.rect.width, window.rect.height],
     "layer": window.layer, "alpha": window.alpha]
}

func readLaunch(_ url: URL) -> [String: Any]? {
    guard let data = try? Data(contentsOf: url) else { return nil }
    return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
}

let args = CommandLine.arguments
guard args.count >= 3 else { fail("Usage: window run-directory startup|inspect|activate|snap|click|quit|terminate|finder ...") }
guard CGPreflightScreenCaptureAccess() else { fail("Screen Recording is unavailable") }
let run = URL(fileURLWithPath: args[1], isDirectory: true).standardizedFileURL
guard run.deletingLastPathComponent().lastPathComponent == "recovery-actions-real-01" else {
    fail("Run path is outside this gate")
}
let command = args[2]
let launchFile = run.appendingPathComponent("launch.json")
let isFinder = command == "finder"
let captureSymbol = dlsym(dlopen(nil, RTLD_LAZY), "CGWindowListCreateImage")
guard let captureSymbol else { fail("Window capture API unavailable") }
typealias Capture = @convention(c) (CGRect, UInt32, CGWindowID, UInt32) -> Unmanaged<CGImage>?
let capture = unsafeBitCast(captureSymbol, to: Capture.self)

func image(_ window: Window) -> CGImage? {
    capture(.null, CGWindowListOption.optionIncludingWindow.rawValue, window.id,
            CGWindowImageOption([.boundsIgnoreFraming, .bestResolution]).rawValue)?.takeRetainedValue()
}

func writePNG(_ image: CGImage, _ url: URL) {
    guard !FileManager.default.fileExists(atPath: url.path),
          let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
    else { fail("Cannot create evidence " + url.path) }
    CGImageDestinationAddImage(dest, image, nil)
    guard CGImageDestinationFinalize(dest) else { fail("PNG encoding failed") }
}

func signature(_ image: CGImage) -> String {
    let width = 240, height = 156
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                            bytesPerRow: width * 4, space: space,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.interpolationQuality = .medium
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    return SHA256.hash(data: Data(bytes: context.data!, count: width * height * 4))
        .map { String(format: "%02x", $0) }.joined()
}

if command == "startup" {
    guard !FileManager.default.fileExists(atPath: launchFile.path) else { fail("Recorder requires a fresh run") }
    let arms = run.appendingPathComponent("startup.armed")
    try Data("ready\n".utf8).write(to: arms, options: .withoutOverwriting)
    emit("armed", ["uptime_ns": DispatchTime.now().uptimeNanoseconds])
    let armedDeadline = DispatchTime.now().uptimeNanoseconds + 12_000_000_000
    var launch: [String: Any]?
    while DispatchTime.now().uptimeNanoseconds < armedDeadline {
        launch = readLaunch(launchFile)
        if launch != nil { break }
        usleep(1800)
    }
    guard let launched = launch, let pid = launched["pid"] as? Int32,
          let start = launched["started_uptime_ns"] as? UInt64,
          let bundle = launched["bundle_id"] as? String,
          let path = launched["app"] as? String,
          bundle.hasPrefix("com.matou.recovery-actions-real-01."),
          path.hasPrefix(run.path + "/") else { fail("No verified isolated launch record") }
    let startup = run.appendingPathComponent("startup", isDirectory: true)
    try FileManager.default.createDirectory(at: startup, withIntermediateDirectories: false)
    let queue = DispatchQueue(label: "recovery PNG", qos: .utility)
    var lastHash: String?
    var lastName = ""
    var frames = 0
    var absent = 0
    var first: UInt64?
    var lastAbsent: UInt64?
    var seenID: CGWindowID?
    while DispatchTime.now().uptimeNanoseconds < start + 8_000_000_000 {
        let begin = DispatchTime.now().uptimeNanoseconds
        if let first, begin > first + 1_200_000_000 { break }
        let matching = windows().filter { $0.pid == pid && $0.layer == 0 && $0.alpha > 0 && $0.rect.width > 300 && $0.rect.height > 180 }
        guard matching.count <= 1 else { fail("Ambiguous isolated startup window") }
        if let w = matching.first {
            guard let app = NSRunningApplication(processIdentifier: pid), app.bundleIdentifier == bundle,
                  app.bundleURL?.path == path, !app.isTerminated else { fail("Startup window process identity differs") }
            if let seenID, seenID != w.id { fail("Startup window ID changed") }
            seenID = w.id
            if first == nil {
                first = begin
                emit("first_window", identity(w).merging(["last_no_window_uptime_ns": lastAbsent as Any? ?? NSNull()],
                                                       uniquingKeysWith: { a, _ in a }))
            }
            if let frame = image(w) {
                let end = DispatchTime.now().uptimeNanoseconds
                let hash = signature(frame)
                if hash != lastHash {
                    lastName = String(format: "frame-%04d.png", frames)
                    let url = startup.appendingPathComponent(lastName)
                    queue.async { writePNG(frame, url) }
                    lastHash = hash
                }
                emit("frame", identity(w).merging(["sample_begin_uptime_ns": begin,
                    "sample_end_uptime_ns": end, "after_start_begin_ms": Double(begin - start) / 1e6,
                    "after_start_end_ms": Double(end - start) / 1e6, "png": lastName,
                    "signature_240x156_sha256": hash], uniquingKeysWith: { a, _ in a }))
                frames += 1
            } else {
                emit("capture_unavailable", ["sample_begin_uptime_ns": begin, "window_id": w.id])
            }
        } else {
            absent += 1
            lastAbsent = begin
        }
        let elapsed = DispatchTime.now().uptimeNanoseconds - begin
        if elapsed < 8_333_333 { usleep(useconds_t((8_333_333 - elapsed) / 1000)) }
    }
    queue.sync {}
    emit("done", ["frames": frames, "no_window_polls": absent, "last_no_window_uptime_ns": lastAbsent as Any? ?? NSNull()])
    exit(frames == 0 ? 2 : 0)
}

let launched = readLaunch(launchFile) ?? [:]
let running: NSRunningApplication
let title: String
if isFinder {
    guard args.count >= 6, let pid = Int32(args[3]),
          let finder = NSRunningApplication(processIdentifier: pid), finder.bundleIdentifier == "com.apple.finder",
          finder.bundleURL?.path == "/System/Library/CoreServices/Finder.app" else { fail("Finder identity differs") }
    running = finder
    title = args[5]
} else {
    guard let pid = launched["pid"] as? Int32, let path = launched["app"] as? String,
          let bundle = launched["bundle_id"] as? String, let expectedTitle = launched["title"] as? String,
          bundle.hasPrefix("com.matou.recovery-actions-real-01."), path.hasPrefix(run.path + "/"),
          let app = NSRunningApplication(processIdentifier: pid), !app.isTerminated,
          app.bundleIdentifier == bundle, app.bundleURL?.path == path else { fail("Isolated process identity differs") }
    running = app
    title = expectedTitle
}

let pid = running.processIdentifier
let windowIndex = isFinder ? 4 : 3
let expectedID = args.count > windowIndex ? CGWindowID(args[windowIndex]) : nil
let operation = isFinder ? (args.count > 6 ? args[6] : "inspect") : command

func checked(_ requireFront: Bool = true, at point: CGPoint? = nil) -> Window {
    let visible = windows()
    let matching = visible.filter { $0.pid == pid && $0.layer == 0 && $0.alpha > 0 && $0.title == title &&
        (expectedID == nil || expectedID == $0.id) }
    guard !running.isTerminated, matching.count == 1, let selected = matching.first else {
        emit("owned_windows", ["windows": visible.filter { $0.pid == pid }.map(identity)])
        fail("Expected exact on-screen window is missing or ambiguous")
    }
    guard !requireFront || NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
        fail("Target process is not frontmost")
    }
    if let point {
        // Dock includes a full-display layer-20 tracking window on this Mac.
        // Its bounds are geometric only. Keep checking other overlapping windows.
        let display = CGDisplayBounds(CGMainDisplayID())
        let enclosing = visible.filter { $0.alpha > 0 && $0.rect.contains(point) }
        let hit = enclosing.first {
            let dockTracking = $0.layer == 20 && $0.rect == display && point.y < display.maxY - 100 &&
                NSRunningApplication(processIdentifier: $0.pid)?.bundleIdentifier == "com.apple.dock"
            let cursor = $0.layer == Int(CGWindowLevelForKey(.cursorWindow)) && $0.title == "Cursor" &&
                $0.rect.width <= 128 && $0.rect.height <= 128
            return !dockTracking && !cursor
        }
        guard selected.rect.contains(point),
              point.y >= selected.rect.minY + 40,
              hit?.id == selected.id else {
            emit("click_obstruction", ["at_screen_pt": [point.x, point.y],
                "blocking_window_id": hit?.id as Any? ?? NSNull(),
                "blocking_owner_pid": hit?.pid as Any? ?? NSNull(),
                "blocking_layer": hit?.layer as Any? ?? NSNull()])
            fail("Click point is outside or occluded in the verified window")
        }
    }
    return selected
}

func targetFile(_ basename: String) -> URL {
    guard basename.hasSuffix(".png"),
          basename.unicodeScalars.allSatisfy({ CharacterSet.alphanumerics.contains($0) || "-_.".unicodeScalars.contains($0) }),
          !basename.hasPrefix(".") else { fail("Invalid evidence PNG name") }
    return run.appendingPathComponent(basename)
}

if operation == "activate" {
    let window = checked(false)
    guard expectedID != nil else { fail("Window ID is required") }
    _ = running.activate(options: [])
    for _ in 0..<30 where NSWorkspace.shared.frontmostApplication?.processIdentifier != pid { usleep(10000) }
    emit("activated", identity(checked()))
    _ = window
    exit(0)
}

let w = checked(operation != "inspect")
emit("identity", identity(w))
if operation == "inspect" { exit(0) }
guard expectedID == w.id else { fail("Explicit window ID required") }

if operation == "snap" {
    let index = isFinder ? 7 : 4
    guard args.count == index + 1, let frame = image(w) else { fail("Screenshot unavailable") }
    writePNG(frame, targetFile(args[index]))
    exit(0)
}

if operation == "terminate" {
    guard !isFinder else { fail("Finder may only close its verified test window") }
    _ = checked()
    guard running.terminate() else { fail("Isolated application did not accept termination request") }
    emit("terminate_requested", ["pid": pid, "window_id": w.id])
    for _ in 0..<300 {
        if running.isTerminated {
            emit("terminated", ["pid": pid])
            exit(0)
        }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
    }
    fail("Isolated application remains running after termination request")
}

guard CGPreflightPostEventAccess() else { fail("System event posting is unavailable") }
let eventSource = CGEventSource(stateID: .hidSystemState)
if operation == "quit" || operation == "close" {
    guard (isFinder && operation == "close") || (!isFinder && operation == "quit") else { fail("Wrong application action") }
    let code: CGKeyCode = isFinder ? 13 : 12
    guard let down = CGEvent(keyboardEventSource: eventSource, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: eventSource, virtualKey: code, keyDown: false) else { fail("Could not create key events") }
    _ = checked()
    down.flags = .maskCommand
    up.flags = .maskCommand
    down.post(tap: .cghidEventTap)
    usleep(8000)
    up.post(tap: .cghidEventTap)
    emit("key_posted", ["window_id": w.id, "pid": pid, "key": isFinder ? "cmd-w" : "cmd-q"])
    exit(0)
}

guard !isFinder, operation == "click", args.count == 7,
      let x = Double(args[4]), let y = Double(args[5]) else { fail("Expected click id x y label") }
let prefix = args[6]
let point = CGPoint(x: w.rect.minX + x, y: w.rect.minY + y)
let guarded = checked(at: point)
if let before = image(guarded) { writePNG(before, targetFile(prefix + "-before.png")) }
guard let move = CGEvent(mouseEventSource: eventSource, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: .left),
      let down = CGEvent(mouseEventSource: eventSource, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left),
      let up = CGEvent(mouseEventSource: eventSource, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)
else { fail("Could not create mouse events") }
_ = checked(at: point)
move.post(tap: .cghidEventTap)
usleep(10000)
_ = checked(at: point)
down.setIntegerValueField(.mouseEventClickState, value: 1)
up.setIntegerValueField(.mouseEventClickState, value: 1)
let start = DispatchTime.now().uptimeNanoseconds
down.post(tap: .cghidEventTap)
up.post(tap: .cghidEventTap)
emit("click_posted", ["window_id": w.id, "owner_pid": pid, "local_pt": [x, y],
                      "screen_pt": [point.x, point.y], "mousedown_uptime_ns": start])
let queue = DispatchQueue(label: "click PNG", qos: .utility)
var previous = ""
for index in 0..<35 {
    let begin = DispatchTime.now().uptimeNanoseconds
    if let active = windows().first(where: { $0.id == w.id && $0.pid == pid && $0.layer == 0 }),
       let after = image(active) {
        let end = DispatchTime.now().uptimeNanoseconds
        let hash = signature(after)
        if hash != previous {
            let name = prefix + String(format: "-%03d.png", index)
            let url = targetFile(name)
            queue.async { writePNG(after, url) }
            emit("changed_frame", ["png": name, "after_click_begin_ms": Double(begin - start) / 1e6,
                                  "after_click_end_ms": Double(end - start) / 1e6])
            previous = hash
        }
    }
    usleep(9000)
}
queue.sync {}
