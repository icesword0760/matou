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
if scenario.startswith('reauth-'):
    initial = {"accounts": [{**a, "needsLogin": True}, b], "currentId": a["id"]}
if scenario.startswith('usage-'):
    a = {**a, "usage": {**a["usage"], "fiveHour": {"used":38,"resetsAt":"2026-10-14T08:00:00Z"},"weekly":{"used":62,"resetsAt":"2026-10-17T08:00:00Z"}}}
    b = {**b, "usage": {**b["usage"], "fiveHour": {"used":12,"resetsAt":"2026-10-14T08:00:00Z"},"weekly":{"used":24,"resetsAt":"2026-10-17T08:00:00Z"}}}
    initial = {"accounts": [a] if scenario in ('usage-single','usage-expired') else [a,b], "currentId": a["id"]}
    if scenario == 'usage-expired': initial["accounts"] = [{**a,"needsLogin":True}]
value = json.loads(path.read_text()) if path.exists() else initial
request = json.loads(sys.stdin.readline())
action = request["action"]
with (root / "actions-fixture.jsonl").open('a') as log:
    log.write(json.dumps({"action": action})+'\n')
result = value
if action == 'refresh':
    if scenario == 'usage-network' and not (root / 'allow-refresh-success').exists():
        print(json.dumps({'ok':False,'error':'engine-error'}));sys.exit(0)
    if scenario == 'usage-slow': time.sleep(3)
    if scenario == 'usage-rate-limit':
        value['accounts'] = [{**x, 'usageIssue':'rate-limited', 'usage':{**x['usage'],'stale':True}} for x in value['accounts']]
if action == "login":
    if scenario in ("browser-missing", "browser-closed"):
        print(json.dumps({"ok": False, "error": scenario})); sys.exit(0)
    if scenario in ('manual', 'manual-failed', 'reauth-manual'):
        submitted = json.loads(sys.stdin.readline())
        assert submitted['nonce'] == request['nonce']
        assert submitted['action'] == 'submit-login-code'
        assert submitted['code'] == 'fixture_code_123456#fixture_state_123456'
        time.sleep(.25)
        if scenario == 'manual-failed':
            print(json.dumps({'ok': False, 'error': 'login-failed'})); sys.exit(0)
    else:
        time.sleep(.35)
    candidate = next((x for x in [a,b] if x["id"] == request.get("replaceId")), b)
    if scenario == 'reauth-wrong': candidate = b
    result = {"draft": candidate}
elif action == "save-draft":
    candidate = next((x for x in [a,b] if x["id"] == request.get("id")), b)
    value["accounts"] = [x for x in value["accounts"] if x["id"] != candidate["id"]] + [candidate]
elif action == "save-current":
    value["accounts"] = [a]; value["currentId"] = a["id"]; value.pop("detected", None)
elif action in ("switch", "reauth"):
    time.sleep(.15)
    value["currentId"] = request["id"]
elif action == "remove":
    value["accounts"] = [x for x in value["accounts"] if x["id"] != request["id"]]
    if value.get("currentId") == request["id"]:
        value.pop("currentId", None); value["detected"] = a
path.write_text(json.dumps(value))
print(json.dumps({"ok": True, "value": {"engineVersion": "fixture", **result}}))
