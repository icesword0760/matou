"""Create symmetric guarded gestures from the live window and measured thumb anchor."""

import argparse
import json
from pathlib import Path


ABC = "com.apple.keylayout.ABC"
SOGOU = "com.sogou.inputmethod.sogou.pinyin"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("launch", type=Path)
    parser.add_argument("phase", choices=("position", "continuous", "hold", "wheel", "outside", "recovery", "fade",
                                         "output", "ime", "ime-control", "ime-ni-check", "ime-cancel", "ime-held-confirmed"))
    parser.add_argument("window", type=int)
    parser.add_argument("window_origin", type=float, nargs=2, metavar=("LEFT", "TOP"))
    parser.add_argument("thumb", type=float, nargs=2, metavar=("X", "Y"))
    parser.add_argument("--outside-x", type=float)
    parser.add_argument("--away-x", type=float, help="Point outside the terminal scrollport and inside the app window")
    parser.add_argument("--target-y", type=float)
    parser.add_argument("--capture", type=float, nargs=4)
    parser.add_argument("--ime-method", choices=("system-key", "key"), default="system-key")
    args = parser.parse_args()
    launch = json.loads(args.launch.read_text())
    x, y = args.thumb
    left, top = args.window_origin
    outside = args.outside_x if args.outside_x is not None else left + 1214
    steps = []

    def add(at, op, **fields):
        steps.append({"at": at, "op": op, **fields})

    def pointer(at, op, dy=0, dx=0):
        add(at, op, x=x + dx, y=y + dy)

    def snap(at, name):
        add(at, "snap", name=f"{args.phase}-{name}.png")

    def grip():
        pointer(0, "move")
        snap(180, "before")
        pointer(250, "down")
        snap(320, "held")

    def type_pinyin(start, text):
        for index, char in enumerate(text):
            if args.ime_method == "system-key":
                values = {"char": char}
            else:
                values = {"code": {"n": 45, "i": 34, "h": 4, "a": 0, "o": 31}[char]}
            add(start + index * 190, args.ime_method, **values)

    phase = args.phase
    if phase not in ("fade", "ime-control", "ime-ni-check", "ime-cancel"):
        grip()
    if phase == "position":
        if args.target_y is None:
            parser.error("position requires --target-y")
        delta = args.target_y - y
        pointer(380, "drag", delta)
        snap(480, "mid-held")
        pointer(555, "up", delta)
        snap(650, "mid")
    if phase == "continuous":
        changes = [*range(1, 11), *range(9, -1, -1)]
        for number, dy in enumerate(changes, 1):
            at = 400 + 115 * number
            pointer(at, "drag", dy)
            snap(at + 14, f"{number:02d}-early")
            snap(at + 67, f"{number:02d}-settled")
        pointer(2820, "up")
        snap(2890, "released")
    if phase == "hold":
        snap(560, "still-before-output")
        add(660, "emit", label="hold", count=16)
        snap(725, "still-output")
        snap(1250, "still-one-second")
        pointer(1350, "up")
        snap(1450, "released")
    if phase == "wheel":
        add(410, "wheel", x=x, y=y, lines=2)
        snap(540, "held-after-wheel")
        pointer(650, "drag", 5)
        snap(750, "continued")
        pointer(850, "up", 5)
        snap(950, "released")
        add(1050, "wheel", x=x, y=y + 5, lines=2)
        snap(1190, "unheld-control")
    if phase == "outside":
        pointer(420, "drag", 4)
        snap(510, "inside-drag")
        pointer(620, "drag", 6, outside - x)
        snap(720, "outside-held")
        pointer(820, "up", 6, outside - x)
        snap(920, "outside-released")
        pointer(1020, "move", 6)
        snap(1220, "returned")
    if phase == "recovery":
        pointer(420, "drag", -6)
        snap(540, "moved")
        pointer(660, "up", -6)
        snap(760, "released")
    if phase == "fade":
        pointer(0, "move")
        snap(220, "hover-baseline")
        away = args.away_x if args.away_x is not None else x + 100
        pointer(300, "move", dx=away - x)
        for delay in (40, 90, 140, 190, 240, 300, 400, 500, 700, 1000, 1400, 2000, 2700):
            snap(300 + delay, f"away-{delay:04d}")
        pointer(3100, "move")
        for delay in (15, 65, 115, 165, 215, 275, 360, 500, 800):
            snap(3100 + delay, f"return-{delay:04d}")
    if phase == "output":
        pointer(400, "drag", 2)
        snap(470, "before-output")
        add(550, "emit", label="drag", count=200)
        for index, at in enumerate(range(600, 1020, 65), 1):
            pointer(at, "drag", 2 + index)
            snap(at + 15, f"during-{index:02d}")
        snap(1100, "after-output-held")
        pointer(1200, "up", 9)
        snap(1300, "released")
        snap(1540, "settled")
    if phase in ("ime", "ime-control"):
        if phase == "ime":
            pointer(425, "drag", 3)
            snap(525, "before-switch")
        else:
            snap(100, "before-switch")
        add(650, "input", id=SOGOU)
        snap(850, "after-switch")
        type_pinyin(1000, "nihao")
        snap(2040, "preedit-early")
        snap(2340, "preedit")
        add(2540, args.ime_method, code=53)
        snap(2810, "escaped")
        if phase == "ime":
            pointer(2950, "up", 3)
            snap(3070, "released")
        add(3210, "input", id=ABC)
        add(3490, "system-key", char="x")
        snap(3760, "recovered")
    if phase == "ime-ni-check":
        snap(100, "before")
        type_pinyin(250, "ni")
        snap(850, "candidate")
        add(900, "observe")
    if phase == "ime-cancel":
        add(150, args.ime_method, code=53)
        snap(400, "escaped")
        add(500, "observe")
    if phase == "ime-held-confirmed":
        pointer(425, "drag", 3)
        snap(550, "history")
        type_pinyin(700, "nihao")
        snap(1700, "candidate")
        add(1760, "observe")
        add(2120, args.ime_method, code=53)
        snap(2460, "escaped")
        add(2520, "observe")
        pointer(2700, "up", 3)
        snap(2800, "released")
        add(2990, "input", id=ABC)
        add(3250, "system-key", char="x")
        snap(3510, "recovered")
    print(json.dumps({"pid": launch["pid"], "app": launch["app"], "title": launch["title"],
                      "window_id": args.window, "rect": args.capture or [left, top, 1200, 780], "steps": steps},
                     ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
