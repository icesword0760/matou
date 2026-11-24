#!/usr/bin/env python3
"""Execute the frozen Chromium selection-autoscroll geometry for a history scrollport."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
VERSION = "150.0.7871.224"
UPSTREAM = f"https://raw.githubusercontent.com/chromium/chromium/{VERSION}/"
CACHE = ROOT / "artifacts/native-parity/reference/history-autoscroll"
FIXTURE = ROOT / "apps/native/desktop/fixtures/history-autoscroll.json"
BLINK = "third_party/blink/renderer/"
PATHS = {
    name: BLINK + path for name, path in {
        "layout_box": "core/layout/layout_box.cc",
        "layout_object": "core/layout/layout_object.cc",
        "selection": "core/page/autoscroll_controller.cc",
        "mouse": "core/input/mouse_event_manager.cc",
        "scroll_into_view": "core/scroll/scroll_into_view_util.cc",
        "alignment": "core/scroll/scroll_alignment.cc",
        "layer": "core/paint/paint_layer_scrollable_area.cc",
        "scrollable_area": "core/scroll/scrollable_area.cc",
        "widget": "platform/widget/widget_base.cc",
        "frame_widget": "core/frame/web_frame_widget_impl.cc",
        "offset": "platform/geometry/physical_offset.h",
        "unit": "platform/geometry/layout_unit.h",
    }.items()
}
PATHS.update({
    "point": "ui/gfx/geometry/point_conversions.cc",
    "vector": "ui/gfx/geometry/vector2d_conversions.cc",
})


def load_sources():
    CACHE.mkdir(parents=True, exist_ok=True)
    sources = {}
    provenance = {}
    for name, path in PATHS.items():
        cached = CACHE / Path(path).name
        if not cached.exists():
            with urllib.request.urlopen(UPSTREAM + path, timeout=30) as response:
                cached.write_bytes(response.read())
        data = cached.read_bytes()
        sources[name] = data.decode("utf-8")
        provenance[name] = {"path": path, "sha256": hashlib.sha256(data).hexdigest()}
    return sources, provenance


def extract(source, start, end):
    first = source.index(start)
    return source[first:source.index(end, first)].strip()


def function(source, signature):
    return extract(source, signature, "\n}") + "\n}"


# Narrow type adapters for one top-level frame, an already identified content
# scrollport (overlay scrollbars excluded), zero scroll margins/padding and no
# scroll snap. The direction, alignment, floor, rounding, and clamp expressions
# used with those adapters come from the extracted Chromium bodies below.
ADAPTERS = r"""
#include <algorithm>
#include <cmath>
#include <compare>
#include <cstdint>
#include <iomanip>
#include <iostream>
#define BUILDFLAG(flag) 0
#define NOT_DESTROYED()
#define DEFINE_STATIC_LOCAL(type, name, args) static type name args
namespace base { int ClampFloor(float value) { return std::floor(value); } }
namespace gfx {
template <typename T> struct XY {
  T vx{}, vy{};
  T x() const { return vx; }
  T y() const { return vy; }
  void set_x(T x) { vx = x; }
  void set_y(T y) { vy = y; }
  void Offset(T x, T y) { vx += x; vy += y; }
  XY operator-(XY other) const { return {vx - other.vx, vy - other.vy}; }
  void SetToMin(XY other) { vx = std::min(vx, other.vx); vy = std::min(vy, other.vy); }
  void SetToMax(XY other) { vx = std::max(vx, other.vx); vy = std::max(vy, other.vy); }
};
using Point = XY<int>;
using PointF = XY<float>;
using Vector2d = XY<int>;
using Vector2dF = XY<float>;
PointF ScalePoint(PointF point, float scale) { return {point.x() * scale, point.y() * scale}; }
}  // namespace gfx

struct LayoutUnit {
  static constexpr int kFixedPointDenominator = 1 << FRACTIONAL_BITS;
  int32_t value = 0;
  LayoutUnit() = default;
  LayoutUnit(float number) : value(number * kFixedPointDenominator) {}
  float ToFloat() const { return static_cast<float>(value) / kFixedPointDenominator; }
  auto operator<=>(const LayoutUnit&) const = default;
  friend bool operator<(float a, LayoutUnit b) { return a < b.ToFloat(); }
  friend bool operator>(float a, LayoutUnit b) { return a > b.ToFloat(); }
  void SetRawValue(int32_t raw) { value = raw; }
  static LayoutUnit FromRawValueWithClamp(int32_t raw) { LayoutUnit result; result.SetRawValue(raw); return result; }
  LayoutUnit operator+(LayoutUnit other) const { return FromRawValueWithClamp(value + other.value); }
  LayoutUnit operator-(LayoutUnit other) const { return FromRawValueWithClamp(value - other.value); }
  LayoutUnit operator/(int divisor) const { return FromRawValueWithClamp(value / divisor); }
UNIT_FUNCTIONS
};

struct PhysicalOffset {
  using PhysicalFixedOffset = PhysicalOffset;
  using ValueType = LayoutUnit;
  LayoutUnit left, top;
  PhysicalOffset() = default;
  PhysicalOffset(LayoutUnit x, LayoutUnit y) : left(x), top(y) {}
  explicit PhysicalOffset(gfx::Point p) : left(p.x()), top(p.y()) {}
  PhysicalOffset operator+(PhysicalOffset other) const { return {left + other.left, top + other.top}; }
  PhysicalOffset operator-() const { return {LayoutUnit() - left, LayoutUnit() - top}; }
OFFSET_FUNCTIONS
};

using ScrollOffset = gfx::Vector2dF;
struct PhysicalBoxStrut {};
struct PhysicalSize { LayoutUnit width, height; };
struct PhysicalRect {
  PhysicalOffset origin;
  PhysicalSize size;
  LayoutUnit X() const { return origin.left; }
  LayoutUnit Y() const { return origin.top; }
  LayoutUnit Right() const { return X() + Width(); }
  LayoutUnit Bottom() const { return Y() + Height(); }
  LayoutUnit Width() const { return size.width; }
  LayoutUnit Height() const { return size.height; }
  void SetWidth(LayoutUnit width) { size.width = width; }
  void SetHeight(LayoutUnit height) { size.height = height; }
  void Move(PhysicalOffset offset) { origin = origin + offset; }
  void Expand(PhysicalBoxStrut) {}
  void ContractEdges(LayoutUnit top, LayoutUnit right, LayoutUnit bottom, LayoutUnit left) {
    Move({left, top});
    SetWidth(Width() - left - right);
    SetHeight(Height() - top - bottom);
  }
};
struct LocalFrameView {
  PhysicalRect ConvertToRootFrame(PhysicalRect rect) { return rect; }
  PhysicalOffset ConvertFromRootFrame(PhysicalOffset offset) { return offset; }
};
struct LocalFrame {
  LocalFrameView view;
  LocalFrameView* View() { return &view; }
  LocalFrameView* GetFrameView() { return &view; }
};
constexpr int kExcludeOverlayScrollbarSizeForHitTesting = 0;
constexpr int kExcludeScrollbars = 0;

struct LayoutBox {
  PhysicalRect bounds;
  mutable LocalFrame frame;
  LocalFrame* GetFrame() const { return &frame; }
  LocalFrame* View() const { return &frame; }
  PhysicalRect AbsoluteBoundingBoxRect() const {
    // LayoutObject::AbsoluteBoundingBoxRect encloses the top-level quad.
    const float left = std::floor(bounds.X().ToFloat()), top = std::floor(bounds.Y().ToFloat());
    return {{left, top}, {std::ceil(bounds.Right().ToFloat()) - left, std::ceil(bounds.Bottom().ToFloat()) - top}};
  }
  void ExcludeScrollbars(PhysicalRect&, int) const {}
  PhysicalOffset CalculateAutoscrollDirection(const gfx::PointF&) const;
};

namespace mojom::blink {
struct ScrollAlignment {
  enum class Behavior { kNoScroll, kClosestEdge, kRight, kBottom, kCenter };
  Behavior rect_visible, rect_hidden, rect_partial;
};
}  // namespace mojom::blink
struct ScrollAlignment { static const mojom::blink::ScrollAlignment& ToEdgeIfNeeded(); };
enum class PhysicalAxis { kHorizontal, kVertical };
struct ScrollableArea {
  PhysicalSize size;
  ScrollOffset current;
  gfx::Vector2d maximum;
  ScrollOffset GetScrollOffset() const { return current; }
  gfx::Vector2d MaximumScrollOffsetInt() const { return maximum; }
  gfx::Vector2d MinimumScrollOffsetInt() const { return {}; }
  PhysicalOffset LocalToScrollOriginOffset() const {
    return PhysicalOffset::FromVector2dFFloor(GetScrollOffset());
  }
  PhysicalRect VisibleScrollSnapportRect(int) const {
    return {PhysicalOffset::FromPointFRound(current), size};
  }
  gfx::Vector2d ClampScrollOffset(const gfx::Vector2d&) const;
};

struct WidgetBase {
  float scale;
  float GetOriginalDeviceScaleFactor() const { return scale; }
  gfx::PointF DIPsToBlinkSpace(const gfx::PointF&);
};

struct WebMouseEvent {
  gfx::PointF position;
  gfx::PointF PositionInRootFrame() const { return position; }
};
"""

MAIN = r"""
int main() {
  float left, top, width, height, x, y, current_x, current_y, max_x, max_y, scale;
  std::cout << std::setprecision(9);
  while (std::cin >> left >> top >> width >> height >> x >> y >>
         current_x >> current_y >> max_x >> max_y >> scale) {
    const PhysicalRect bounds{{left * scale, top * scale}, {width * scale, height * scale}};
    LayoutBox layout_box{bounds};
    ScrollableArea area{bounds.size, {-current_x * scale, -current_y * scale},
                        {static_cast<int>(max_x * scale), static_cast<int>(max_y * scale)}};
    const WebMouseEvent event{WidgetBase{scale}.DIPsToBlinkSpace({x, y})};
    PhysicalOffset last_known_mouse_position_in_root_frame_;
MOUSE_POSITION
    struct EventHandler {
      gfx::PointF position;
      gfx::PointF LastKnownMousePositionInRootFrame() const { return position; }
    } event_handler{{last_known_mouse_position_in_root_frame_.left.ToFloat(),
                     last_known_mouse_position_in_root_frame_.top.ToFloat()}};
    auto* autoscroll_layout_object_ = &layout_box;
SELECTION_POINT
    // Convert the target from the root frame to the history scrollport's box.
    const PhysicalOffset absolute_position = selection_point + -bounds.origin;
    const PhysicalRect local_expose_rect(
TARGET_RECT
    );
    const auto& alignment = ScrollAlignment::ToEdgeIfNeeded();
    const ScrollOffset target_offset = GetScrollOffsetToExpose(
        area, local_expose_rect, {}, alignment, alignment);
    const auto next = area.ClampScrollOffset(gfx::ToRoundedVector2d(target_offset));
    std::cout << -next.x() / scale << " " << -next.y() / scale << "\n";
  }
}
"""


def harness(src):
    fractional_bits = re.search(r"using LayoutUnit = FixedPoint<(\d+), int32_t>;", src["unit"])[1]
    unit = extract(src["unit"], "static FixedPoint FromFloatFloor", "static FixedPoint FromDoubleRound")
    unit = unit.replace("static FixedPoint", "static LayoutUnit")
    offset = extract(src["offset"], "static PhysicalFixedOffset FromPointFRound", "  void Scale(")
    constants = extract(src["layout_box"], "#if BUILDFLAG(IS_IOS)\nstatic constexpr int kAutoscrollBeltSize", "static const unsigned kBackground")
    selection_start = src["selection"].index("void AutoscrollController::Animate()")
    pieces = [
        ADAPTERS.replace("FRACTIONAL_BITS", fractional_bits)
        .replace("UNIT_FUNCTIONS", unit)
        .replace("OFFSET_FUNCTIONS", offset),
        "namespace gfx {",
        function(src["point"], "Point ToFlooredPoint"),
        function(src["vector"], "Vector2d ToRoundedVector2d"),
        "}",
        constants,
        function(src["layout_box"], "PhysicalOffset LayoutBox::CalculateAutoscrollDirection"),
        function(src["widget"], "gfx::PointF WidgetBase::DIPsToBlinkSpace"),
        function(src["alignment"], "const mojom::blink::ScrollAlignment& ScrollAlignment::ToEdgeIfNeeded"),
        extract(src["scroll_into_view"], "struct LayoutRange {", "V8ScrollLogicalPosition::Enum ResolveAutoAlignment"),
        function(src["scroll_into_view"], "ScrollOffset GetScrollOffsetToExpose"),
        function(src["scrollable_area"], "gfx::Vector2d ScrollableArea::ClampScrollOffset"),
        MAIN.replace("MOUSE_POSITION", extract(src["mouse"], "last_known_mouse_position_in_root_frame_ =\n", "  last_known_mouse_position_ ="))
        .replace("SELECTION_POINT", extract(src["selection"][selection_start:], "PhysicalOffset offset =", "  switch (autoscroll_type_)"))
        .replace("TARGET_RECT", extract(src["layout_box"], "PhysicalRect(absolute_position,\n", ",\n      std::move(params), nullptr, false, true);")),
    ]
    return "\n\n".join(pieces)


# Coordinates use logical pixels. Offsets use the native negative sign.
VIEWPORT = (30, 40, 230, 180)
SCROLLED = (-70, -90)
MAXIMUM = (300, 250)
PROBES = (
    ("interior", (110, 135)),
    ("top-at-20-logical", (110, 60)),
    ("top-just-inside-20-logical", (110, 59.75)),
    ("top-at-10-logical", (110, 50)),
    ("top-just-inside-10-logical", (110, 49.75)),
    ("bottom-at-20-logical", (110, 200)),
    ("bottom-fractional-20-logical", (110, 200.75)),
    ("bottom-just-inside-20-logical", (110, 201)),
    ("bottom-at-10-logical", (110, 210)),
    ("bottom-just-inside-10-logical", (110, 210.5)),
    ("left-belt", (37, 135)),
    ("right-belt", (251, 135)),
    ("far-bottom-right", (298.5, 284.5)),
)
FRACTIONAL_VIEWPORT = (30.375, 40.375, 230.375, 180.375)
EXTRA_PROBES = (
    ("far-top-left", VIEWPORT, (-16, -70), (-205, -200), MAXIMUM),
    ("clamp-to-start", VIEWPORT, (-16, -70), (-6, -5), MAXIMUM),
    ("clamp-to-end", VIEWPORT, (298.5, 284.5), (-296, -247), MAXIMUM),
    ("horizontal-axis-without-overflow", VIEWPORT, (298.5, 284.5), (0, -90), (0, 250)),
    ("fractional-origin-top-left", FRACTIONAL_VIEWPORT, (37.25, 49.25), SCROLLED, MAXIMUM),
    ("fractional-size-bottom-right", FRACTIONAL_VIEWPORT, (271, 211.5), SCROLLED, MAXIMUM),
    ("fractional-scroll-interior", VIEWPORT, (110, 135), (-70.25, -90.75), MAXIMUM),
    ("fractional-scroll-near-edges", FRACTIONAL_VIEWPORT, (272, 211.5), (-70.25, -90.75), MAXIMUM),
)


def main():
    sources, provenance = load_sources()
    geometry = [(name, VIEWPORT, pointer, SCROLLED, MAXIMUM) for name, pointer in PROBES]
    cases = [
        {
            "name": f"{name}-{scale}x",
            "viewport": viewport,
            "pointer": pointer,
            "offset": offset,
            "maximum": maximum,
            "scale": scale,
        }
        for scale in (1, 2)
        for name, viewport, pointer, offset, maximum in [*geometry, *EXTRA_PROBES]
    ]
    raw_input = "\n".join(
        " ".join(map(str, [*case["viewport"], *case["pointer"], *case["offset"], *case["maximum"], case["scale"]]))
        for case in cases
    )
    with tempfile.TemporaryDirectory(prefix="matou-history-autoscroll-") as tmp:
        program = Path(tmp) / "history-autoscroll.cc"
        binary = program.with_suffix("")
        program.write_text(harness(sources), encoding="utf-8")
        subprocess.run([os.environ.get("CXX", "c++"), "-std=c++20", str(program), "-o", str(binary)], check=True)
        output = subprocess.run([binary], input=raw_input, text=True, capture_output=True, check=True).stdout
    for case, line in zip(cases, output.splitlines(), strict=True):
        case["next"] = [float(value) for value in line.split()]
    fixture = {
        "chromium": VERSION,
        "upstream": UPSTREAM,
        "sources": provenance,
    }
    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    rows = ",\n".join(f"    {json.dumps(case)}" for case in cases)
    prefix = json.dumps(fixture, indent=2).removesuffix("\n}")
    FIXTURE.write_text(f'{prefix},\n  "cases": [\n{rows}\n  ]\n}}\n', encoding="utf-8")
    print(f"Generated {len(cases)} cases in {FIXTURE.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
