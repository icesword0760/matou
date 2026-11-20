import AppKit
import Carbon
import CoreGraphics
import Darwin

func now() -> UInt64 { clock_gettime_nsec_np(CLOCK_REALTIME) }

func emit(_ event: String, _ fields: [String: Any] = [:]) {
    var row = fields
    row["event"] = event
    row["unix_ns"] = now()
    let bytes = try! JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes + Data([10]))
}

func fail(_ reason: String) -> Never {
    emit("refused", ["reason": reason])
    exit(1)
}

let args = CommandLine.arguments
guard args.count >= 3,
      let data = try? Data(contentsOf: URL(fileURLWithPath: args[1])),
      let launch = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
      let path = launch["app"] as? String, let bundle = launch["bundle_id"] as? String,
      let pid = launch["pid"] as? Int32, let title = launch["title"] as? String,
      path.contains("/evidence/recovery-water-real-03/"),
      bundle.hasPrefix("com.matou.recovery-water-real-03."),
      let running = NSRunningApplication(processIdentifier: pid), !running.isTerminated,
      running.bundleIdentifier == bundle, running.bundleURL?.path == path else {
    fail("Isolated PID, bundle identifier or bundle path mismatch")
}

let command = args[2]
let expectedWindow = args.count > 3 ? CGWindowID(args[3]) : nil
let ownerKey = kCGWindowOwnerPID as String
let titleKey = kCGWindowName as String
let idKey = kCGWindowNumber as String
let boundsKey = kCGWindowBounds as String

func checkedWindow() -> [String: Any]? {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], .zero)
        as? [[String: Any]] ?? []
    let normal = list.filter {
        $0[kCGWindowLayer as String] as? Int == 0 && ($0[kCGWindowAlpha as String] as? Double ?? 0) > 0
    }
    let matching = normal.filter { $0[ownerKey] as? Int32 == pid && $0[titleKey] as? String == title }
    guard !running.isTerminated, matching.count == 1, let window = matching.first,
          let id = window[idKey] as? CGWindowID, expectedWindow == nil || id == expectedWindow else { return nil }
    if command == "inspect" { return window }
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { return nil }
    return window
}

func currentSource() -> String {
    guard let source = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
          let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return "" }
    return Unmanaged<CFString>.fromOpaque(id).takeUnretainedValue() as String
}

guard CGPreflightScreenCaptureAccess() else { fail("Screen Capture permission unavailable") }
guard let window = checkedWindow(),
      let id = window[idKey] as? CGWindowID,
      let bounds = window[boundsKey] as? [String: Double], let x = bounds["X"], let y = bounds["Y"],
      let width = bounds["Width"], let height = bounds["Height"] else {
    fail("Exact on-screen window PID, ID, title or foreground guard failed")
}
let source = currentSource()
emit("identity", ["window_id": id, "owner_pid": pid, "title": title, "bundle": bundle,
                  "app": path, "bounds_pt": bounds, "input_source": source,
                  "frontmost_pid": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1])
if command == "inspect" || command == "guard" { exit(0) }
guard expectedWindow != nil, CGPreflightPostEventAccess() else { fail("Event posting unavailable or window ID missing") }

func key(_ code: CGKeyCode, flags: CGEventFlags = []) {
    guard checkedWindow() != nil else { fail("Foreground identity changed before keyboard event") }
    guard flags.contains(.maskCommand) || ["com.apple.keylayout.ABC", "com.apple.keylayout.US"].contains(currentSource())
        else { fail("Select the ABC or U.S. input source for an unmodified Latin key") }
    let keyboard = CGEventSource(stateID: .hidSystemState)
    guard let down = CGEvent(keyboardEventSource: keyboard, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: keyboard, virtualKey: code, keyDown: false) else {
        fail("Cannot construct system keyboard event")
    }
    down.flags = flags
    up.flags = flags
    if flags.isEmpty, code == 0 || code == 7 {
        var character: UniChar = code == 0 ? 0x61 : 0x78
        down.keyboardSetUnicodeString(stringLength: 1, unicodeString: &character)
        up.keyboardSetUnicodeString(stringLength: 1, unicodeString: &character)
    }
    let posted = now()
    down.post(tap: .cghidEventTap)
    usleep(8000)
    up.post(tap: .cghidEventTap)
    emit("key_posted", ["keydown_unix_ns": posted, "keycode": code, "flags": flags.rawValue,
                       "window_id": id, "owner_pid": pid, "input_source": currentSource()])
}

switch command {
case "a": key(0)
case "x": key(7)
case "quit": key(12, flags: .maskCommand)
case "watch-x":
    guard args.count == 7, let localX = Double(args[4]), let localY = Double(args[5]),
          localX > 20, localX < width - 20, localY > 20, localY < height - 20,
          let captureSymbol = dlsym(dlopen(nil, RTLD_LAZY), "CGWindowListCreateImage") else {
        fail("Invalid water sampling location or capture API unavailable")
    }
    typealias Capture = @convention(c) (CGRect, UInt32, CGWindowID, UInt32) -> Unmanaged<CGImage>?
    let capture = unsafeBitCast(captureSymbol, to: Capture.self)
    let captureOptions = CGWindowListOption.optionIncludingWindow.rawValue
    let imageOptions = CGWindowImageOption([.boundsIgnoreFraming, .bestResolution]).rawValue
    let triggerFile = URL(fileURLWithPath: args[6]).standardizedFileURL
    guard triggerFile.deletingLastPathComponent().path == URL(fileURLWithPath: args[1])
            .standardizedFileURL.deletingLastPathComponent().path,
          triggerFile.lastPathComponent == "crash.json",
          !FileManager.default.fileExists(atPath: triggerFile.path) else { fail("Stale or invalid trigger file") }
    emit("watch_armed", ["point_x": localX, "point_y": localY])
    let deadline = DispatchTime.now().uptimeNanoseconds + 20_000_000_000
    while !FileManager.default.fileExists(atPath: triggerFile.path) {
        if DispatchTime.now().uptimeNanoseconds >= deadline { fail("Timed out waiting for certified crash") }
        usleep(1000)
    }
    let stop = DispatchTime.now().uptimeNanoseconds + 2_000_000_000
    while DispatchTime.now().uptimeNanoseconds < stop {
        guard checkedWindow() != nil else { fail("Foreground identity changed while waiting for water") }
        let sampleBegin = now()
        guard let image = capture(.null, captureOptions, id, imageOptions)?.takeRetainedValue() else {
            fail("Could not capture verified window")
        }
        let sampleEnd = now()
        let samples = [-10.0, 0, 10].flatMap { offsetX in
            [-10.0, 0, 10].compactMap { offsetY -> [UInt8]? in
                let px = Int((localX + offsetX) * Double(image.width) / width)
                let py = Int((localY + offsetY) * Double(image.height) / height)
                guard let crop = image.cropping(to: CGRect(x: px, y: py, width: 1, height: 1)) else { return nil }
                var rgba = [UInt8](repeating: 0, count: 4)
                rgba.withUnsafeMutableBytes { bytes in
                    let rgb = CGColorSpace(name: CGColorSpace.sRGB)!
                    if let context = CGContext(data: bytes.baseAddress, width: 1, height: 1, bitsPerComponent: 8,
                                               bytesPerRow: 4, space: rgb,
                                               bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) {
                        context.draw(crop, in: CGRect(x: 0, y: 0, width: 1, height: 1))
                    }
                }
                return rgba
            }
        }
        let blue = samples.filter { pixel in
            let r = Int(pixel[0]), g = Int(pixel[1]), b = Int(pixel[2])
            return r >= 218 && r <= 251 && b >= 238 && b - r >= 5 && g - r >= 1
        }
        if blue.count >= 6 {
            emit("water_detected", ["sample_begin_unix_ns": sampleBegin, "sample_end_unix_ns": sampleEnd,
                                    "rgba": samples, "matching": blue.count])
            key(7)
            exit(0)
        }
        usleep(3500)
    }
    fail("No matching water sampled. No key was sent.")
case "click":
    guard args.count == 6, let localX = Double(args[4]), let localY = Double(args[5]),
          (0..<width).contains(localX), (0..<height).contains(localY), checkedWindow() != nil else {
        fail("Click lies outside the verified window")
    }
    let point = CGPoint(x: x + localX, y: y + localY)
    guard let down = CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left),
          let up = CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)
    else { fail("Cannot construct system click") }
    down.setIntegerValueField(.mouseEventClickState, value: 1)
    up.setIntegerValueField(.mouseEventClickState, value: 1)
    let posted = now()
    down.post(tap: .cghidEventTap)
    up.post(tap: .cghidEventTap)
    emit("click_posted", ["mousedown_unix_ns": posted, "local_x": localX, "local_y": localY,
                         "window_id": id, "owner_pid": pid])
default: fail("Unrecognized UI operation")
}
