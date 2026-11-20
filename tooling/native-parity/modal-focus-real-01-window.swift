// Real-window evidence helper for the two disposable modal-focus bundles.
import AppKit
import ApplicationServices
import Darwin
import ImageIO
import UniformTypeIdentifiers

func stop(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

func json(_ value: Any) {
    guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) else {
        stop("Unable to encode evidence")
    }
    FileHandle.standardOutput.write(data + Data([10]))
}

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func point(_ element: AXUIElement) -> CGPoint? {
    guard let value = attribute(element, kAXPositionAttribute), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var result = CGPoint.zero
    return AXValueGetValue(value as! AXValue, .cgPoint, &result) ? result : nil
}

func axSize(_ element: AXUIElement) -> CGSize? {
    guard let value = attribute(element, kAXSizeAttribute), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
    var result = CGSize.zero
    return AXValueGetValue(value as! AXValue, .cgSize, &result) ? result : nil
}

func rectJSON(_ rect: CGRect) -> [String: CGFloat] {
    ["x": rect.minX, "y": rect.minY, "width": rect.width, "height": rect.height]
}

func axInfo(_ element: AXUIElement?) -> [String: Any] {
    guard let element else { return [:] }
    var result: [String: Any] = [:]
    var owner: pid_t = 0
    if AXUIElementGetPid(element, &owner) == .success { result["pid"] = Int(owner) }
    for name in [kAXRoleAttribute, kAXSubroleAttribute, kAXTitleAttribute, kAXDescriptionAttribute,
                 kAXIdentifierAttribute, kAXHelpAttribute, kAXFocusedAttribute, kAXEnabledAttribute,
                 kAXSelectedTextAttribute, kAXValueAttribute] {
        if let value = attribute(element, name) as? String {
            result[name] = String(value.prefix(200))
        } else if let value = attribute(element, name) as? NSNumber {
            result[name] = value
        }
    }
    if let selected = attribute(element, kAXSelectedTextRangeAttribute), CFGetTypeID(selected) == AXValueGetTypeID() {
        var range = CFRange(location: 0, length: 0)
        if AXValueGetValue(selected as! AXValue, .cfRange, &range) {
            result["selected_range"] = ["location": range.location, "length": range.length]
        }
    }
    if let origin = point(element), let size = axSize(element) {
        result["frame"] = rectJSON(CGRect(origin: origin, size: size))
    }
    return result
}

func axElement(_ value: CFTypeRef?) -> AXUIElement? {
    guard let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return (value as! AXUIElement)
}

struct Window {
    let id: Int
    let owner: Int
    let layer: Int
    let title: String
    let bounds: CGRect
    let alpha: Double
    let onScreen: Bool

    var record: [String: Any] {
        ["id": id, "owner_pid": owner, "title": title, "layer": layer,
         "bounds_pt": rectJSON(bounds), "on_screen": onScreen]
    }
}

func windows(onScreenOnly: Bool = false) -> [Window] {
    let option: CGWindowListOption = onScreenOnly ? .optionOnScreenOnly : .optionAll
    return (CGWindowListCopyWindowInfo([option, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? [])
        .compactMap { row in
            guard let id = row[kCGWindowNumber as String] as? Int,
                  let owner = row[kCGWindowOwnerPID as String] as? Int,
                  let layer = row[kCGWindowLayer as String] as? Int,
                  let raw = row[kCGWindowBounds as String] as? [String: Any],
                  let bounds = CGRect(dictionaryRepresentation: raw as CFDictionary) else { return nil }
            return Window(id: id, owner: owner, layer: layer,
                          title: row[kCGWindowName as String] as? String ?? "", bounds: bounds,
                          alpha: row[kCGWindowAlpha as String] as? Double ?? 0,
                          onScreen: row[kCGWindowIsOnscreen as String] as? Bool ?? false)
        }
}

let args = CommandLine.arguments
guard args.count >= 3 else { stop("Usage: modal-focus-ui launch.json inspect | <id> <operation> [arguments]") }
let launchURL = URL(fileURLWithPath: args[1]).standardizedFileURL
let evidenceSegment = "/artifacts/native-parity/native/evidence/modal-focus-real-01/"
guard let data = try? Data(contentsOf: launchURL),
      let record = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let rawPID = record["pid"] as? Int, rawPID > 1, rawPID <= Int(Int32.max),
      let appPath = record["app"] as? String, let bundle = record["bundle"] as? String,
      let title = record["title"] as? String, let side = record["side"] as? String,
      ["native", "reference"].contains(side),
      let segmentRange = launchURL.path.range(of: evidenceSegment, options: .backwards),
      [side + "/launch.json", "recovery/" + side + "/launch.json", "fork/" + side + "/launch.json"].contains(String(launchURL.path[segmentRange.upperBound...])),
      URL(fileURLWithPath: appPath).deletingLastPathComponent() == launchURL.deletingLastPathComponent(),
      bundle == "com.matou.modal-focus-real-01." + (launchURL.path.hasSuffix("/recovery/" + side + "/launch.json") ? "recovery." :
                   launchURL.path.hasSuffix("/fork/" + side + "/launch.json") ? "fork." : "") + side,
      title == (side == "native" ? "Matou Native" : "码头") else { stop("Not a modal-focus launch record") }
let pid = pid_t(rawPID)
guard let app = NSRunningApplication(processIdentifier: pid),
      app.bundleURL?.path == appPath, app.bundleIdentifier == bundle else {
    stop("Isolated PID, path, and bundle ID do not match")
}
let axApp = AXUIElementCreateApplication(pid)
let axSystem = AXUIElementCreateSystemWide()
func ownedAXWindow(_ window: Window) -> AXUIElement? {
    let list = attribute(axApp, kAXWindowsAttribute) as? [AXUIElement] ?? []
    return list.first { element in
        guard attribute(element, kAXTitleAttribute) as? String == title,
              let origin = point(element), let size = axSize(element) else { return false }
        return abs(origin.x - window.bounds.minX) <= 1 && abs(origin.y - window.bounds.minY) <= 1
            && abs(size.width - window.bounds.width) <= 1 && abs(size.height - window.bounds.height) <= 1
    }
}
func front() -> Int { Int(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1) }

if args[2] == "inspect" {
    let display = CGMainDisplayID()
    let owned = windows().filter { $0.owner == rawPID && $0.layer == 0 }
    let session = CGSessionCopyCurrentDictionary() as? [String: Any] ?? [:]
    json(["side": side, "pid": rawPID, "bundle_path": appPath, "front_pid": front(),
          "screen_access": CGPreflightScreenCaptureAccess(), "event_access": CGPreflightPostEventAccess(),
          "ax_access": AXIsProcessTrusted(), "console": session["kCGSSessionOnConsoleKey"] ?? NSNull(),
          "locked": session["CGSSessionScreenIsLocked"] ?? NSNull(),
          "display_pt": rectJSON(CGDisplayBounds(display)), "windows": owned.map(\.record),
          "ax_windows": (attribute(axApp, kAXWindowsAttribute) as? [AXUIElement] ?? []).map { axInfo($0) }])
    exit(0)
}
guard args.count >= 4, let windowID = Int(args[2]), windowID > 0 else { stop("Missing exact CG window ID") }
let operation = args[3]

func target(requireFront: Bool = true) -> Window {
    guard !app.isTerminated, app.bundleURL?.path == appPath, app.bundleIdentifier == bundle,
          let window = windows().first(where: { $0.id == windowID && $0.owner == rawPID && $0.layer == 0 }),
          window.title == title else { stop("Exact PID, CG window ID, owner PID, and title did not match") }
    if requireFront && (front() != rawPID || !window.onScreen) { stop("Isolated target is not frontmost and on screen") }
    return window
}

func pointer(_ window: Window) -> CGPoint {
    guard args.count >= 6, let x = Double(args[4]), let y = Double(args[5]), x.isFinite, y.isFinite else {
        stop("Requires window-local x y in points")
    }
    let position = CGPoint(x: window.bounds.minX + x, y: window.bounds.minY + y)
    let hit = windows(onScreenOnly: true).first { $0.alpha > 0 && $0.layer == 0 && $0.bounds.contains(position) }
    guard window.bounds.contains(position), let hit, hit.owner == rawPID && hit.id == windowID else {
        let seen = hit.map { "owner \($0.owner), window \($0.id)" } ?? "no candidate"
        stop("Pointer does not resolve to exact trial window (\(seen))")
    }
    return position
}

func axHit(_ position: CGPoint) -> [String: Any] {
    var element: AXUIElement?
    let error = AXUIElementCopyElementAtPosition(axSystem, Float(position.x), Float(position.y), &element)
    let info = axInfo(element)
    return ["status": error.rawValue, "element": (info["pid"] as? Int == rawPID) ? info : [:]]
}

func snapshot(_ window: Window) -> [String: Any] {
    ["window": window.record, "front_pid": front(), "bundle_path": appPath,
     "ax_window": axInfo(ownedAXWindow(window)),
     "focus": axInfo(axElement(attribute(axApp, kAXFocusedUIElementAttribute)))]
}

func mouseEvent(_ type: CGEventType, _ position: CGPoint, clicks: Int = 1) {
    guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: position,
                              mouseButton: type == .rightMouseDown || type == .rightMouseUp ? .right : .left) else {
        stop("Cannot create mouse event")
    }
    event.setIntegerValueField(.mouseEventClickState, value: Int64(clicks))
    event.post(tap: .cghidEventTap)
}

func keyboard(_ code: Int, flags: CGEventFlags = [], chars: [UInt16] = []) {
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: CGKeyCode(code), keyDown: down) else {
            stop("Cannot create key event")
        }
        event.flags = flags
        if !chars.isEmpty { event.keyboardSetUnicodeString(stringLength: chars.count, unicodeString: chars) }
        event.post(tap: .cghidEventTap)
    }
}

func capture(_ window: Window, path: String) -> [String: Any] {
    let destination = URL(fileURLWithPath: path).standardizedFileURL
    guard destination.path.hasPrefix(launchURL.deletingLastPathComponent().path + "/"),
          destination.pathExtension == "png", !FileManager.default.fileExists(atPath: destination.path),
          CGPreflightScreenCaptureAccess() else { stop("Invalid isolated screenshot destination or screen access") }
    typealias Capture = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
    guard let symbol = dlsym(dlopen(nil, RTLD_LAZY), "CGWindowListCreateImage") else { stop("CG capture unavailable") }
    let take = unsafeBitCast(symbol, to: Capture.self)
    let options: CGWindowImageOption = [.boundsIgnoreFraming, .bestResolution]
    let image: CGImage
    if operation == "screen-snap" {
        guard windows(onScreenOnly: true).filter({ $0.layer == 0 && $0.alpha > 0 })
            .prefix(while: { $0.id != window.id }).allSatisfy({ !$0.bounds.intersects(window.bounds) || $0.owner == rawPID }),
              let captured = take(window.bounds, CGWindowListOption.optionOnScreenOnly.rawValue, 0, options.rawValue)?.takeRetainedValue()
        else { stop("Overlapping external window or screen capture failed") }
        image = captured
    } else {
        guard let captured = take(.null, CGWindowListOption.optionIncludingWindow.rawValue, UInt32(window.id),
                                  options.rawValue)?.takeRetainedValue() else { stop("Window capture failed") }
        image = captured
    }
    guard let writer = CGImageDestinationCreateWithURL(destination as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        stop("Cannot open screenshot destination")
    }
    CGImageDestinationAddImage(writer, image, nil)
    guard CGImageDestinationFinalize(writer) else { stop("Cannot write screenshot") }
    return ["file": destination.lastPathComponent, "width_px": image.width, "height_px": image.height]
}

let before = target(requireFront: operation != "activate")
var result: [String: Any] = ["operation": operation, "time": ISO8601DateFormatter().string(from: Date()),
                             "before": snapshot(before)]
switch operation {
case "activate":
    result["activate_returned"] = app.activate()
    Thread.sleep(forTimeInterval: 0.35)
case "state":
    break
case "resize":
    guard args.count == 6, let w = Double(args[4]), let h = Double(args[5]),
          [520, 700, 720, 900, 979, 980, 1200].contains(w), h == 780, let element = ownedAXWindow(before) else {
        stop("Resize requires an exact AX window matching CG geometry and a trial dimension")
    }
    var size = CGSize(width: w, height: h)
    guard let value = AXValueCreate(.cgSize, &size) else { stop("Cannot create AX resize value") }
    result["requested_pt"] = ["width": w, "height": h]
    result["ax_set_size_status"] = AXUIElementSetAttributeValue(element, kAXSizeAttribute as CFString, value).rawValue
    Thread.sleep(forTimeInterval: 0.65)
case "snap", "screen-snap":
    guard args.count == 5 else { stop("Specify output PNG under the trial directory") }
    result["screenshot"] = capture(before, path: args[4])
case "quit":
    result["terminate_requested"] = app.terminate()
    json(result)
    exit(0)
case "click", "double", "right", "hover", "wheel", "hit":
    let position = pointer(before)
    result["window_point_pt"] = ["x": position.x - before.bounds.minX, "y": position.y - before.bounds.minY]
    result["screen_point_pt"] = ["x": position.x, "y": position.y]
    result["hit_before"] = axHit(position)
    if operation != "hit" {
        guard CGPreflightPostEventAccess() else { stop("No permission to post input") }
        mouseEvent(.mouseMoved, position)
        Thread.sleep(forTimeInterval: 0.08)
        _ = pointer(target())
        switch operation {
        case "click", "double":
            for count in 1...(operation == "double" ? 2 : 1) {
                mouseEvent(.leftMouseDown, position, clicks: count)
                mouseEvent(.leftMouseUp, position, clicks: count)
                Thread.sleep(forTimeInterval: 0.06)
            }
        case "right":
            mouseEvent(.rightMouseDown, position)
            mouseEvent(.rightMouseUp, position)
        case "wheel":
            guard args.count == 8, let dy = Int32(args[6]), let dx = Int32(args[7]),
                  (-1000...1000).contains(dy), (-1000...1000).contains(dx),
                  let event = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2,
                                      wheel1: dy, wheel2: dx, wheel3: 0) else { stop("Specify wheel deltaY and deltaX") }
            event.location = position
            event.post(tap: .cghidEventTap)
            result["scroll_delta"] = ["y": dy, "x": dx]
        default:
            break
        }
        Thread.sleep(forTimeInterval: operation == "hover" ? 0.95 : 0.45)
        _ = target()
        result["hit_after"] = axHit(position)
    }
case "key":
    guard args.count == 5 || args.count == 6, let code = Int(args[4]), (0...127).contains(code),
          CGPreflightPostEventAccess() else { stop("key requires virtual code and optional cmd modifier") }
    let modifiers: [String: CGEventFlags] = ["cmd": .maskCommand, "shift": .maskShift, "ctrl": .maskControl]
    let modifier = args.count == 6 ? args[5] : ""
    guard modifier.isEmpty || modifiers[modifier] != nil else { stop("Unsupported key modifier") }
    keyboard(code, flags: modifiers[modifier] ?? [])
    result["modifier"] = modifier
    result["key_code"] = code
    Thread.sleep(forTimeInterval: 0.3)
case "text":
    guard args.count == 5, args[4].count <= 160, !args[4].contains(where: \.isNewline),
          CGPreflightPostEventAccess() else { stop("text requires a single short fixture label") }
    for character in args[4] { keyboard(0, chars: Array(String(character).utf16)) }
    result["characters_sent"] = args[4].count
    Thread.sleep(forTimeInterval: 0.3)
default:
    stop("Unknown operation")
}
result["after"] = snapshot(target())
json(result)
