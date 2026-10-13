"""Electron acceptance fixture: never imports authentication code or reads HOME."""
import json
import os
from pathlib import Path
import sys
import time
root = Path(sys.argv[1]); root.mkdir(parents=True, exist_ok=True)
path = root / "engine-fixture.json"
a = {"id": "fixture-a", "name": "工作账户", "email": "wo•••@example.com", "plan": "Claude", "needsLogin": False,
     "usage": {"checkedAt": int(time.time()*1000), "stale": False, "fiveHour": {"used": 100, "resetsAt": "2026-10-13T11:40:00Z"}, "weekly": {"used": 48, "resetsAt": "2026-10-16T01:00:00Z"}}}
b = {**a, "id": "fixture-b", "name": "个人账户", "email": "me•••@example.com", "usage": {**a["usage"], "fiveHour": {"used": 32, "resetsAt": "2026-10-13T11:40:00Z"}}}
scenario = os.environ.get("MATOU_E2E_ACCOUNTS_SCENARIO", "multiple")
initial = {"accounts": [a,b], "currentId": a["id"]} if scenario == "multiple" else {"accounts": [], **({"detected": a} if scenario == "detected" else {})}
value = json.loads(path.read_text()) if path.exists() else initial
request = json.loads(sys.stdin.readline())
action = request["action"]
result = value
if action == "login":
    if scenario in ("browser-missing", "browser-closed"):
        print(json.dumps({"ok": False, "error": scenario})); sys.exit(0)
    time.sleep(.35)
    result = {"draft": b}
elif action == "save-draft":
    value["accounts"] = [x for x in value["accounts"] if x["id"] != b["id"]] + [b]
elif action == "save-current":
    value["accounts"] = [a]; value["currentId"] = a["id"]; value.pop("detected", None)
elif action == "switch":
    time.sleep(.15)
    value["currentId"] = request["id"]
elif action == "remove":
    value["accounts"] = [x for x in value["accounts"] if x["id"] != request["id"]]
    if value.get("currentId") == request["id"]:
        value.pop("currentId", None); value["detected"] = a
path.write_text(json.dumps(value))
print(json.dumps({"ok": True, "value": {"engineVersion": "fixture", **result}}))
