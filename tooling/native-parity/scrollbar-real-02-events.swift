// Local evidence fixture. Run only against the isolated acceptance windows.
import AppKit
import ApplicationServices
import Carbon
import CoreGraphics
import Darwin
import ImageIO
import UniformTypeIdentifiers

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func jsonLine(_ value: Any) {
    do {
        let bytes = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys, .fragmentsAllowed])
        FileHandle.standardOutput.write(bytes + Data([10]))
    } catch {
        fail("Cannot serialize evidence: \(error)")
    }
}

func inputID(_ source: TISInputSource) -> String {
    guard let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) else { return "" }
    return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
}

func currentInput() -> String {
    inputID(TISCopyCurrentKeyboardInputSource().takeRetainedValue())
}

func front() -> [String: Any] {
    guard let app = NSWorkspace.shared.frontmostApplication else { return [:] }
    return [
        "pid": Int(app.processIdentifier), "bundle": app.bundleIdentifier ?? "",
        "name": app.localizedName ?? "", "path": app.bundleURL?.path ?? "",
    ]
}

func windows() -> [[String: Any]] {
    (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] ?? []).compactMap { info in
        guard let id = info[kCGWindowNumber as String],
              let pid = info[kCGWindowOwnerPID as String],
              let bounds = info[kCGWindowBounds as String],
              let layer = info[kCGWindowLayer as String] else { return nil }
        return ["id": id, "pid": pid, "bounds": bounds, "layer": layer,
                "title": info[kCGWindowName as String] as? String ?? "",
                "alpha": info[kCGWindowAlpha as String] as? Double ?? 0]
    }
}

let args = CommandLine.arguments
if args.count == 2 && args[1] == "state" {
    let display = CGMainDisplayID()
    let session = CGSessionCopyCurrentDictionary() as? [String: Any] ?? [:]
    jsonLine([
        "front": front(), "input": currentInput(),
        "screenCaptureAllowed": CGPreflightScreenCaptureAccess(),
        "eventPostingAllowed": CGPreflightPostEventAccess(),
        "accessibilityTrusted": AXIsProcessTrusted(),
        "locked": session["CGSSessionScreenIsLocked"] ?? NSNull(),
        "sessionOnConsole": session["kCGSSessionOnConsoleKey"] ?? NSNull(),
        "windows": windows(),
        "screenPoints": ["width": CGDisplayBounds(display).width, "height": CGDisplayBounds(display).height],
        "screenPixels": ["width": CGDisplayPixelsWide(display), "height": CGDisplayPixelsHigh(display)],
    ])
    exit(0)
}

guard args.count == 4, args[1] == "run" else { fail("Usage: window-sequence state | run plan.json output-directory") }
let output = URL(fileURLWithPath: args[3], isDirectory: true)
let planData = try Data(contentsOf: URL(fileURLWithPath: args[2]))
guard let plan = try JSONSerialization.jsonObject(with: planData) as? [String: Any],
      let steps = plan["steps"] as? [[String: Any]],
      let rect = plan["rect"] as? [Double], rect.count == 4 else {
    fail("Plan must contain rect [x,y,width,height] and steps")
}
let region = CGRect(x: rect[0], y: rect[1], width: rect[2], height: rect[3])
if region.width <= 0 || region.height <= 0 { fail("Capture region is empty") }
let defaultPID = plan["pid"] as? Int
let targetWindowID = plan["window_id"] as? Int
guard let expectedTitle = plan["title"] as? String,
      ["Matou Native", "码头"].contains(expectedTitle),
      let expectedApp = plan["app"] as? String,
      expectedApp.contains("/evidence/scrollbar-real-01/02/"),
      expectedApp.hasSuffix(".app") else { fail("Plan requires the isolated bundle path and exact title") }
let runDirectory = URL(fileURLWithPath: expectedApp).deletingLastPathComponent()
let allowInput = steps.contains { ["move", "down", "drag", "up", "wheel", "key", "system-key"].contains($0["op"] as? String ?? "") }
if allowInput && !CGPreflightPostEventAccess() { fail("Current process does not have permission to post native events") }

typealias Capture = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
guard let captureSymbol = dlsym(dlopen(nil, RTLD_LAZY), "CGWindowListCreateImage") else {
    fail("CGWindowListCreateImage unavailable")
}
let capture = unsafeBitCast(captureSymbol, to: Capture.self)
let captureOptions = CGWindowListOption.optionOnScreenOnly.rawValue
let imageOptions = CGWindowImageOption.bestResolution.rawValue
let start = DispatchTime.now().uptimeNanoseconds
let encodeQueue = DispatchQueue(label: "evidence.png")
var mouse = CGEvent(source: nil)?.location ?? .zero
var leftDown = false
var pointBeforeDown: CGPoint?
var heldNumber = Int64(1)
let source = CGEventSource(stateID: .hidSystemState)
source?.localEventsSuppressionInterval = 0

func elapsed() -> Double { Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000 }

func mouseEvent(_ type: CGEventType, _ point: CGPoint, recoveryPID: Int? = nil) {
    guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left) else {
        fail("Cannot create mouse event")
    }
    if type == .leftMouseDown || type == .leftMouseDragged || type == .leftMouseUp {
        event.setIntegerValueField(.mouseEventNumber, value: heldNumber)
        event.setIntegerValueField(.mouseEventClickState, value: 1)
    }
    event.setIntegerValueField(.mouseEventDeltaX, value: Int64((point.x - mouse.x).rounded()))
    event.setIntegerValueField(.mouseEventDeltaY, value: Int64((point.y - mouse.y).rounded()))
    if let recoveryPID {
        event.postToPid(pid_t(recoveryPID))
    } else {
        event.post(tap: .cghidEventTap)
    }
    mouse = point
}

func recoverButton() {
    if leftDown {
        if let pid = defaultPID,
           NSRunningApplication(processIdentifier: pid_t(pid))?.bundleURL?.path == expectedApp {
            mouseEvent(.leftMouseUp, mouse, recoveryPID: pid)
        }
        leftDown = false
    }
}

func checkedFailure(_ message: String) -> Never {
    recoverButton()
    encodeQueue.sync {}
    fail(message)
}

// Reject captured or injected samples if an unrelated application owns the
// foreground or an unrelated normal window intersects our target above it.
func windowBounds(_ info: [String: Any]) -> CGRect? {
    guard let fields = info["bounds"] as? [String: Any] else { return nil }
    return CGRect(dictionaryRepresentation: fields as CFDictionary)
}

func normalWindow(_ info: [String: Any]) -> Bool {
    (info["layer"] as? Int) == 0 && (info["alpha"] as? Double ?? 0) > 0
}

func checkedWindow(_ pid: Int, _ expectedID: Int, _ operation: String, point: CGPoint? = nil) -> [String: Any] {
    guard (front()["pid"] as? Int) == pid, (front()["path"] as? String) == expectedApp else {
        checkedFailure("Unexpected frontmost PID during \(operation), expected \(pid): \(front())")
    }
    let onScreen = windows()
    guard let index = onScreen.firstIndex(where: {
        ($0["id"] as? Int) == expectedID && ($0["pid"] as? Int) == pid
    }) else {
        checkedFailure("Expected CG window \(expectedID) for \(pid) is absent during \(operation)")
    }
    let verified = onScreen[index]
    guard verified["title"] as? String == expectedTitle else {
        checkedFailure("Window title changed during \(operation): \(verified)")
    }
    guard let target = windowBounds(verified), target.width == 1200, target.height == 780,
          target.contains(region) else {
        checkedFailure("Target geometry changed during \(operation)")
    }
    for above in onScreen[..<index] where normalWindow(above) {
        if let other = windowBounds(above), target.intersects(other), other.width > 0, other.height > 0 {
            checkedFailure("Overlapping normal window during \(operation): \(above)")
        }
    }
    if let point {
        let hit = onScreen.first { normalWindow($0) && windowBounds($0)?.contains(point) == true }
        if target.contains(point) {
            guard hit?["id"] as? Int == expectedID else {
                checkedFailure("Pointer would hit another window during \(operation): \(String(describing: hit))")
            }
        } else {
            let permitted = operation == "move" || (leftDown && ["drag", "up"].contains(operation))
            guard permitted, CGDisplayBounds(CGMainDisplayID()).contains(point) else {
                checkedFailure("Pointer outside the target during \(operation): \(point)")
            }
        }
    }
    return verified
}

func key(_ code: Int, _ flags: CGEventFlags = []) {
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(code), keyDown: down) else {
            checkedFailure("Cannot create keyboard event")
        }
        event.flags = flags
        event.post(tap: .cghidEventTap)
    }
}

func systemKey(_ pid: Int, _ expectedID: Int, code: Int?, character: String?) {
    guard character == nil || ["a", "n", "i", "h", "o", "x"].contains(character!),
          code == nil || (0...127).contains(code!) else { checkedFailure("Invalid physical key") }
    let send = character.map { "keystroke \"\($0)\"" } ?? "key code \(code!)"
    let applescript = """
        tell application "System Events" to set targetState to get {unix id, frontmost, name of every window} of (first application process whose unix id is \(pid))
        log ("AX before key: " & (targetState as text))
        if item 1 of targetState is not \(pid) or item 2 of targetState is not true or item 3 of targetState does not contain \"\(expectedTitle)\" then error "AX PID, foreground or title changed"
        tell application "System Events" to \(send)
        """
    _ = checkedWindow(pid, expectedID, "system-key")
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    process.arguments = ["-e", applescript]
    do { try process.run() } catch { checkedFailure("System Events unavailable: \(error)") }
    process.waitUntilExit()
    if process.terminationStatus != 0 { checkedFailure("System Events rejected the key") }
}

for step in steps {
    guard let at = step["at"] as? Double, let op = step["op"] as? String else { checkedFailure("Step missing at/op") }
    let delay = at - elapsed()
    if delay > 0 { Thread.sleep(forTimeInterval: delay / 1000) }
    let pid = step["pid"] as? Int ?? defaultPID
    let began = elapsed()
    var record: [String: Any] = [
        "op": op, "planned_ms": at, "actual_ms": began, "unix_ms": Date().timeIntervalSince1970 * 1000,
    ]
    if let label = step["label"] { record["label"] = label }
    let point = CGPoint(x: step["x"] as? Double ?? mouse.x, y: step["y"] as? Double ?? mouse.y)
    let pointer = ["move", "down", "drag", "up", "wheel"].contains(op)
    if pointer || ["key", "system-key", "snap", "emit"].contains(op) || (op == "input" && pid != nil) {
        guard let pid, let targetWindowID else {
            checkedFailure("Verification requires both PID and CG window ID for \(op)")
        }
        record["verified_window"] = checkedWindow(pid, targetWindowID, op, point: pointer ? point : nil)
        record["verified_pid"] = pid
    }
    switch op {
    case "move", "down", "drag", "up":
        switch op {
        case "down":
            if leftDown { checkedFailure("Repeated mouse down") }
            heldNumber += 1
            pointBeforeDown = point
            mouseEvent(.leftMouseDown, point)
            leftDown = true
        case "drag":
            if !leftDown { checkedFailure("Drag requested without a held mouse button") }
            mouseEvent(.leftMouseDragged, point)
        case "up":
            if !leftDown { checkedFailure("Release requested without a held mouse button") }
            mouseEvent(.leftMouseUp, point)
            leftDown = false
            pointBeforeDown = nil
        default:
            if leftDown { checkedFailure("Use drag while the left button is held") }
            mouseEvent(.mouseMoved, point)
        }
    case "wheel":
        let lines = step["lines"] as? Int32 ?? 0
        guard let event = CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 1, wheel1: lines, wheel2: 0, wheel3: 0) else {
            checkedFailure("Cannot create scroll event")
        }
        event.location = point
        event.post(tap: .cghidEventTap)
        record["lines"] = lines
    case "key":
        guard let code = step["code"] as? Int, (0...127).contains(code) else { checkedFailure("Invalid virtual keycode") }
        key(code, CGEventFlags(rawValue: step["flags"] as? UInt64 ?? 0))
        record["code"] = code
    case "system-key":
        guard let pid, let targetWindowID else { checkedFailure("System Events requires the target identity") }
        let code = step["code"] as? Int
        let character = step["char"] as? String
        guard (code == nil) != (character == nil) else { checkedFailure("Pass one key code or character") }
        systemKey(pid, targetWindowID, code: code, character: character)
        record["code"] = code
        record["char"] = character
    case "input":
        guard let id = step["id"] as? String,
              let sourceList = TISCreateInputSourceList([kTISPropertyInputSourceID as String: id] as CFDictionary, false)?
                .takeRetainedValue() as? [TISInputSource],
              let newSource = sourceList.first else { checkedFailure("Requested input source unavailable") }
        if currentInput() != id {
            let status = TISSelectInputSource(newSource)
            if status != noErr { checkedFailure("Input source change failed with status \(status)") }
        }
        record["input"] = currentInput()
        if currentInput() != id { checkedFailure("Requested input source was not selected: \(id)") }
    case "emit":
        guard leftDown, let label = step["label"] as? String, ["hold", "drag"].contains(label),
              let count = step["count"] as? Int, (1...200).contains(count) else {
            checkedFailure("Output is limited to the two held gestures")
        }
        let request = runDirectory.appendingPathComponent("emit-\(label).request")
        guard !FileManager.default.fileExists(atPath: request.path) else { checkedFailure("Output request already exists") }
        try String(count).write(to: request, atomically: true, encoding: .utf8)
        record["count"] = count
    case "snap":
        guard let name = step["name"] as? String, !name.contains("/"), name.hasSuffix(".png"),
              let png = capture(region, captureOptions, kCGNullWindowID, imageOptions)?.takeRetainedValue() else {
            checkedFailure("Composited real-display capture failed")
        }
        let destination = output.appendingPathComponent(name)
        if FileManager.default.fileExists(atPath: destination.path) { checkedFailure("Evidence filename already exists: \(name)") }
        record["screenshot"] = name
        record["capture_end_ms"] = elapsed()
        record["size_px"] = [png.width, png.height]
        record["rect_pt"] = rect
        record["front"] = front()
        record["input"] = currentInput()
        encodeQueue.async {
            guard let file = CGImageDestinationCreateWithURL(destination as CFURL, UTType.png.identifier as CFString, 1, nil) else {
                fail("Cannot create \(name)")
            }
            CGImageDestinationAddImage(file, png, nil)
            if !CGImageDestinationFinalize(file) { fail("Cannot encode \(name)") }
        }
    case "observe":
        record["front"] = front()
        record["input"] = currentInput()
        record["windows"] = windows()
    default:
        checkedFailure("Unknown action \(op)")
    }
    record["done_ms"] = elapsed()
    record["mouse_pt"] = [mouse.x, mouse.y]
    record["left_held"] = leftDown
    if let actualMouse = CGEvent(source: nil)?.location {
        record["system_mouse_pt"] = [actualMouse.x, actualMouse.y]
    }
    record["system_left_held"] = CGEventSource.buttonState(.combinedSessionState, button: .left)
    if let pointBeforeDown { record["held_origin_pt"] = [pointBeforeDown.x, pointBeforeDown.y] }
    jsonLine(record)
}
recoverButton()
encodeQueue.sync {}
