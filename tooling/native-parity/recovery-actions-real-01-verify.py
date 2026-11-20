#!/usr/bin/env python3
"""Read back the retained snapshots from the real-window recovery actions."""
import json
import re
from pathlib import Path
import sys

GATE = Path(__file__).resolve().parents[2] / "artifacts/native-parity/native/evidence/recovery-actions-real-01"
DB = "matou.sqlite"
MARKER = f"{DB}.recovery.json"
checks = {}


def files(run, name):
    return json.loads((GATE / run / f"{name}.json").read_text())["files"]


def report(case, points):
    checks[case] = points


def same_bytes(actual, expected):
    return actual.get("sha256") == expected.get("sha256") and actual.get("size") == expected.get("size")


def preserved_parked(after, before):
    old = [key for key in before if key.startswith(DB + ".corrupt-")]
    return len(old) > 0 and all(same_bytes(after[key], before[key]) for key in [MARKER, *old])


def integrity(row):
    return row["sqlite_immutable"].get("integrity") == [["ok"]]


def sentinel(row, text):
    return row["sqlite_immutable"].get("sentinel") == [[text]]


def workspace(row, name):
    return any(item[1] == name for item in row["sqlite_immutable"].get("workspaces", []))


def settlements(rows):
    return [row["json"] for key, row in rows.items() if key.startswith(MARKER + ".resolved-") and "json" in row]


def without_settlement(rows):
    return not settlements(rows) and rows[MARKER]["json"]["state"] == "required"


def settled(rows):
    marker = rows[MARKER]["json"]
    return len(settlements(rows)) == 1 and all(
        row["state"] == "resolved" and row["recoveryId"] == marker["recoveryId"]
        for row in settlements(rows)
    )


def candidates(rows):
    pairs = {}
    for filename, info in rows.items():
        if filename.startswith("backups/") and filename.endswith(".json"):
            data = info["json"]
            binary = rows[f'backups/{data["id"]}.sqlite']
            pairs[data["id"]] = (
                data["size"] == binary["size"] and data["sha256"] == binary["sha256"]
                and integrity(binary) and Path(data["path"]).name == f'{data["id"]}.sqlite'
            )
    return pairs


def first_frames():
    native = files("native-first-r2", "before-launch")[DB]
    native_end = files("native-first-r2", "after-startup")
    reference = files("reference-first", "before-launch")[DB]
    reference_end = files("reference-first", "after-startup")
    sides = [native_end, reference_end]
    report("same_corruption", {
        "identical_input_header_damage": same_bytes(native, reference),
        "both_preserved_corrupt_bytes":
            all(any(same_bytes(row, native) for key, row in side.items()
                    if key.startswith(DB + ".corrupt-")) for side in sides),
        "both_markers_identify_physical_corruption":
            all(side[MARKER]["json"]["reason"] == "physical-corruption" for side in sides),
    })


def recheck():
    bad = files("native-first", "after-bad-recheck")
    before = files("native-first", "before-launch")
    good = files("native-first", "after-action")
    report("recheck", {
        "bad_click_left_original_missing_and_recovery_pending": DB not in bad and without_settlement(bad),
        "initial_bad_bytes_quarantined": any(same_bytes(row, before[DB]) for key, row in bad.items()
                                             if key.startswith(DB + ".corrupt-")),
        "good_click_preserved_marker_and_quarantine": preserved_parked(good, bad),
        "good_click_opened_labelled_workspace": integrity(good[DB])
            and sentinel(good[DB], "R01 Rechecked Workspace") and workspace(good[DB], "R01 Rechecked Workspace"),
        "good_click_persisted_single_resolved_generation": settled(good),
    })


def restore(failure):
    run = "native-postfail" if failure else "native-restore"
    initial = files(run, "before-launch")
    before = files(run, "staged-original")
    after = files(run, "after-action")
    rollback = [row for key, row in after.items() if key.startswith(DB + ".replaced-")]
    original = "R01 Original Before Failure" if failure else "R01 Original Before Restore"
    expected = "R01 Backup Poisoned" if failure else "R01 Backup Alternate"
    valid = candidates(initial)
    points = {
        "both_initial_backup_files_pass_size_sha256_and_sqlite_integrity":
            len(valid) == 2 and all(valid.values()),
        "active_db_matches_user_clicked_backup": integrity(after[DB])
            and sentinel(after[DB], expected) and workspace(after[DB], expected),
        "active_db_differs_from_original": not same_bytes(after[DB], before[DB]),
        "readable_replaced_copy_contains_original": len(rollback) == 1
            and integrity(rollback[0]) and sentinel(rollback[0], original) and workspace(rollback[0], original),
        "marker_and_quarantine_bytes_preserved": preserved_parked(after, before),
        "input_candidates_byte_preserved": all(same_bytes(after[key], row) for key, row in initial.items()
            if key.startswith("backups/")),
    }
    if failure:
        points["checksum_mismatch_in_replaced_active_db"] = [1, "fixture-post-replace-checksum-mismatch"] in (
            after[DB]["sqlite_immutable"]["schema_migrations"])
        points["failed_click_left_unresolved_marker"] = without_settlement(after)
    else:
        points["successful_click_settled_once"] = settled(after)
    report(run, points)


def empty():
    before = files("native-empty", "staged-original")
    after = files("native-empty", "after-clean-exit")
    archived = [key for key in after if re.fullmatch(r"recovery-evidence/[-a-f\d]{36}/matou\.sqlite", key)]
    points = {
        "one_archive_with_exact_main_wal_shm": len(archived) == 1
            and all(same_bytes(after[archived[0] + suffix], before[DB + suffix])
                    for suffix in ("", "-wal", "-shm")),
        "old_corrupt_bytes_and_marker_preserved": preserved_parked(after, before),
        "new_empty_db_valid_and_without_fixture_content": integrity(after[DB])
            and len(after[DB]["sqlite_immutable"]["workspaces"]) == 1
            and "gate_payload" not in after[DB]["sqlite_immutable"]["tables"]
            and not workspace(after[DB], "R01 Original To Archive"),
        "create_settled_once": settled(after),
    }
    report("empty", points)


def export():
    before = files("native-export", "before-launch")
    after = files("native-export", "after-action")
    marker = after[MARKER]["json"]
    bundles = [key for key in after if re.fullmatch(r"recovery-exports/\d{13}-[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}", key)]
    if len(bundles) != 1:
        report("export", {"timestamp_uuid_directory": False})
        return
    folder, = bundles
    row = after[folder + "/manifest.json"]
    manifest = row["json"]
    quarantine = Path(marker["quarantinedPath"]).name
    expected = [MARKER, quarantine, quarantine + "-wal", quarantine + "-shm"]
    report("export", {
        "timestamp_uuid_directory": True,
        "manifest_fields_and_backup_count": manifest == {
            "exportedAt": manifest["exportedAt"],
            "reason": marker["reason"],
            "durableDatabasePath": marker["durableDatabasePath"],
            "quarantinedPath": marker["quarantinedPath"],
            "backupCount": 0,
            "backups": [],
            "exportedFiles": expected,
        } and isinstance(manifest["exportedAt"], int),
        "manifest_mode_0600": row["mode"] == "0o600",
        "export_tree_exact_filenames": sorted(key[len(folder) + 1:] for key in after if key.startswith(folder + "/"))
            == sorted(expected + ["manifest.json"]),
        "all_copies_match_sources_bytes_modes": all(
            same_bytes(after[name], after[folder + "/" + name]) and
            after[name]["mode"] == after[folder + "/" + name]["mode"] for name in expected),
        "quarantined_main_and_sidecars_match_initial": all(
            same_bytes(after[quarantine + suffix], before[DB + suffix]) for suffix in ("", "-wal", "-shm")),
        "export_did_not_settle_or_create_workspace_db": DB not in after and without_settlement(after),
    })


def auxiliary():
    owner_before = files("native-owner", "before-launch")
    owner_after = files("native-owner", "after-startup")
    marker_before = files("native-marker", "before-launch")
    marker_after = files("native-marker", "after-startup")
    owner_marker = owner_after[MARKER]["json"]
    report("auxiliary", {
        "owner_case_preserved_database_and_original_owner": all(
            same_bytes(owner_after[key], owner_before[key]) for key in (DB, DB + ".owner")),
        "owner_problem_classified": owner_marker["reason"] == "ownership-recovery-required"
            and owner_marker["ownershipIssue"] == "owner-record-malformed",
        "invalid_marker_case_preserved_both_input_files": all(
            same_bytes(marker_after[key], marker_before[key]) for key in (DB, MARKER)),
        "invalid_marker_reported_in_runtime_log": "database recovery marker is invalid"
            in (GATE / "native-marker/app.log").read_text(),
    })


for name, verify in (
    ("same_corruption", first_frames), ("recheck", recheck),
    ("native-restore", lambda: restore(False)), ("native-postfail", lambda: restore(True)),
    ("empty", empty), ("export", export), ("auxiliary", auxiliary),
):
    try:
        verify()
    except (FileNotFoundError, KeyError, TypeError, IndexError, ValueError) as error:
        checks[name] = {"incomplete": str(error)}

print(json.dumps(checks, ensure_ascii=False, indent=2))
sys.exit(0 if all(value is True for stage in checks.values() for value in stage.values()) else 1)
