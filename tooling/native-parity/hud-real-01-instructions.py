"""Change only the tracked instructions fixture for this acceptance run."""
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path
import stat
import sys


def main():
    if (len(sys.argv) not in (3, 4) or sys.argv[1] not in ("native", "reference")
            or len(sys.argv) == 4 and sys.argv[3] != "--workspace"):
        raise SystemExit("Usage: hud-real-01-instructions.py native|reference check|unreadable|missing|restore [--workspace]")
    side, action = sys.argv[1:3]
    run = Path(__file__).resolve().parents[2] / "artifacts/native-parity/native/evidence/hud-real-01" / side
    fixture = json.loads((run / "prepared.json").read_text())
    workspace = len(sys.argv) == 4
    project = Path(fixture["paths"]["workspace"] if workspace else fixture["fixture"]["project"])
    if not project.is_relative_to(run) or project.resolve() != project:
        raise RuntimeError("Fixture cwd is not a local directory of this trial")
    path = project / "AGENTS.md"
    saved = project / ".agents-fixture-away"
    if path.is_symlink() or saved.is_symlink():
        raise RuntimeError("Refusing a linked instruction fixture")
    if action == "unreadable":
        path.chmod(0)
    elif action == "missing":
        if saved.exists():
            raise RuntimeError("Preserving the previous instruction fixture")
        path.chmod(0o644)
        path.rename(saved)
    elif action == "restore":
        if saved.exists():
            if path.exists():
                raise RuntimeError("Unexpected new AGENTS.md at the fixture path")
            saved.rename(path)
        path.chmod(0o644)
    elif action != "check":
        raise SystemExit("Unknown fixture action")
    record = {"time": datetime.now(timezone.utc).isoformat(), "side": side, "action": action,
              "scope": "workspace" if workspace else "codex-project",
              "exists": path.exists(), "readable": os.access(path, os.R_OK)}
    if path.exists():
        record["mode"] = oct(stat.S_IMODE(path.stat().st_mode))
    if record["readable"]:
        record["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        record["matches_preseeded"] = record["sha256"] == fixture["fixture"]["agents_sha256"]
    line = json.dumps(record, ensure_ascii=False)
    with (run / "instructions-fixture.jsonl").open("a", encoding="utf-8") as output:
        output.write(line + "\n")
    print(line)


if __name__ == "__main__":
    main()
