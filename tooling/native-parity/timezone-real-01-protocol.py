"""Exercise the native product runtime's normal settings commands without a window."""
import asyncio
import json
from pathlib import Path
import sqlite3
import time


ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "artifacts/native-parity/native/evidence/timezone-real-01"
RUNTIME = EVIDENCE / "native/MatouNative.app/Contents/MacOS/matou-native-runtime"


async def check():
    run = EVIDENCE / "native-protocol"
    run.mkdir(mode=0o700)
    data, home, work, temporary = (run / name for name in ("data", "home", "workspace", "tmp"))
    for folder in (data, home, work, temporary):
        folder.mkdir(mode=0o700)
    database = data / "matou.sqlite"
    args = [str(RUNTIME), "--data-root", str(data), "--workspace", str(work),
            "--shell-home", str(home), "--shell", "/bin/zsh"]
    env = {"HOME": str(home), "ZDOTDIR": str(home), "TMPDIR": str(temporary) + "/",
           "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "SHELL": "/bin/zsh", "LANG": "C.UTF-8",
           "MATOU_DISABLE_LOGIN_PATH_PROBE": "1"}

    with (run / "transcript.jsonl").open("x") as log, (run / "runtime.stderr").open("xb") as errors:
        process = await asyncio.create_subprocess_exec(
            *args, env=env, cwd=work, stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=errors, limit=8 * 1024 * 1024
        )

        def record(direction, value):
            log.write(json.dumps({"time_ns": time.time_ns(), "direction": direction, "message": value}) + "\n")
            log.flush()

        async def send(value):
            record("send", value)
            process.stdin.write((json.dumps(value) + "\n").encode())
            await process.stdin.drain()

        async def wait_for(predicate):
            async with asyncio.timeout(10):
                while line := await process.stdout.readline():
                    value = json.loads(line)
                    record("receive", value)
                    if predicate(value):
                        return value
            raise RuntimeError("Runtime closed while a reply was expected")

        async def request(request_id, method, **fields):
            await send({"Command": {method: {"request_id": request_id} | fields}})
            message = await wait_for(
                lambda value: isinstance(event := value.get("Event"), dict)
                and event.get("TimeZoneSettings", {}).get("requestId") == request_id
            )
            result = message["Event"]["TimeZoneSettings"]["result"]
            print(json.dumps({"id": request_id, "method": method, "fields": fields,
                              "result": {"Err": result["Err"]} if "Err" in result else
                              {"Ok": {key: value for key, value in result["Ok"].items() if key != "zones"}}}))
            return result

        try:
            await send({"Hello": {"version": 3}})
            assert await wait_for(lambda value: value.get("Hello") is not None) == {"Hello": {"version": 3}}
            await wait_for(lambda value: isinstance(event := value.get("Event"), dict) and "Ready" in event)
            first = await request(1, "GetTimeZoneSettings")
            assert first["Ok"]["timeZone"] == "auto"
            baseline = await request(2, "SetTimeZoneSettings", time_zone="UTC")
            assert baseline["Ok"]["timeZone"] == "UTC"
            for request_id, invalid in enumerate(("Mars/Olympus", "+08:00", 480, None), start=3):
                answer = await request(request_id, "SetTimeZoneSettings", time_zone=invalid)
                assert answer == {"Err": "INVALID_REQUEST"}
            after = await request(7, "GetTimeZoneSettings")
            assert after["Ok"]["timeZone"] == "UTC"
            with sqlite3.connect(database.as_uri() + "?mode=rw", uri=True) as connection:
                connection.execute(
                    "CREATE TRIGGER timezone_real_01_save_fault BEFORE INSERT ON preferences "
                    "WHEN NEW.key = 'claude.timeZone' BEGIN SELECT RAISE(ABORT, 'isolated save fault'); END"
                )
            try:
                assert await request(8, "SetTimeZoneSettings", time_zone="Asia/Tokyo") == {"Err": "STORAGE"}
            finally:
                with sqlite3.connect(database.as_uri() + "?mode=rw", uri=True) as connection:
                    connection.execute("DROP TRIGGER timezone_real_01_save_fault")
            assert (await request(9, "GetTimeZoneSettings"))["Ok"]["timeZone"] == "UTC"
            await send({"Command": "Shutdown"})
            await wait_for(lambda value: value.get("Event") == "Closed")
            assert await asyncio.wait_for(process.wait(), timeout=10) == 0
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        stored, = connection.execute(
            "SELECT value_json FROM preferences WHERE key = ?", ("claude.timeZone",)
        ).fetchone()
        assert stored == '"UTC"'
    print(json.dumps({"database_time_zone": json.loads(stored), "exit": process.returncode}))


if __name__ == "__main__":
    asyncio.run(check())
