// Records exact isolated frontmost startup windows at a short cadence.
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
guard args.count == 3, let minimumLaunchNS = UInt64(args[2]) else {
    fail("Usage: startup-window isolated-side-directory minimum-launch-unix-ns")
}
guard CGPreflightScreenCaptureAccess() else { fail("Screen Recording is unavailable") }
let run = URL(fileURLWithPath: args[1], isDirectory: true).standardizedFileURL
guard run.deletingLastPathComponent().path.hasSuffix("/artifacts/native-parity/native/evidence/timezone-real-01"),
      ["native", "reference"].contains(run.lastPathComponent) else {
    fail("Run path is outside this gate")
}
let launchFile = run.appendingPathComponent("launch.json")
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

do {
    let arms = run.appendingPathComponent("startup-\(minimumLaunchNS).armed")
    try Data("ready\n".utf8).write(to: arms, options: .withoutOverwriting)
    emit("armed", ["uptime_ns": DispatchTime.now().uptimeNanoseconds])
    let armedDeadline = DispatchTime.now().uptimeNanoseconds + 12_000_000_000
    var launch: [String: Any]?
    while DispatchTime.now().uptimeNanoseconds < armedDeadline {
        launch = readLaunch(launchFile)
        if let launch, let time = launch["started_unix_ns"] as? UInt64, time >= minimumLaunchNS { break }
        launch = nil
        usleep(1800)
    }
    guard let launched = launch, let pid = launched["pid"] as? Int32,
          let start = launched["started_uptime_ns"] as? UInt64,
          let bundle = launched["bundle"] as? String,
          let path = launched["app"] as? String, let expectedTitle = launched["title"] as? String,
          bundle == "com.matou.timezone-real-01." + run.lastPathComponent,
          URL(fileURLWithPath: path).deletingLastPathComponent() == run else { fail("No verified isolated launch record") }
    let startup = run.appendingPathComponent("startup-\(minimumLaunchNS)", isDirectory: true)
    try FileManager.default.createDirectory(at: startup, withIntermediateDirectories: false)
    let queue = DispatchQueue(label: "startup PNG", qos: .utility)
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
        if let w = matching.first, w.title == expectedTitle,
           NSWorkspace.shared.frontmostApplication?.processIdentifier == pid {
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
