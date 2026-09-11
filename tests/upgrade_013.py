#!/usr/bin/env python3
"""Offline v0.1.13 -> current data-upgrade fixture. Run after `cargo build`.

The TOML and DDL below are copied from tag v0.1.13 (src/config.rs and
src/usage.rs::Store::open).  Keep this fixture self-contained: release
verification must not depend on Git history or a downloaded old binary.
"""
import argparse
import json
import os
from pathlib import Path
import select
import shutil
import sqlite3
import subprocess
import tempfile
import time


V013_CONFIG = """\
db_path = "usage.db"

[[account]]
provider = "serper"
label = "serper-legacy"
api_key = "fixture-key-not-live"
quota = 2500
reset = "once"

[[account]]
provider = "grok_web"
label = "grok-legacy"
dr_quota = 3
dr_reset = "daily"
"""


V013_DDL = """
CREATE TABLE usage (
    provider TEXT NOT NULL, label TEXT NOT NULL, period TEXT NOT NULL,
    used INTEGER NOT NULL DEFAULT 0, exhausted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (label, period)
);
CREATE TABLE proxy_assignment (label TEXT PRIMARY KEY, proxy TEXT NOT NULL);
CREATE TABLE web_session (
    label TEXT PRIMARY KEY, provider TEXT NOT NULL, cookies TEXT NOT NULL,
    updated TEXT NOT NULL, identity TEXT
);
CREATE TABLE route_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, capability TEXT NOT NULL,
    provider TEXT NOT NULL, label TEXT NOT NULL, status INTEGER NOT NULL,
    latency_ms INTEGER NOT NULL, fail_from TEXT, fail_code INTEGER,
    niche TEXT NOT NULL DEFAULT '', debug_id INTEGER
);
CREATE TABLE debug_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, capability TEXT NOT NULL,
    provider TEXT NOT NULL, label TEXT NOT NULL, status INTEGER NOT NULL,
    latency_ms INTEGER NOT NULL, request TEXT NOT NULL, response TEXT,
    error TEXT, http_trace TEXT
);
"""


def wait_for_mcp(process):
    sequence = 0
    pending = b""

    def request(method, params):
        nonlocal sequence, pending
        sequence += 1
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": sequence,
                                        "method": method, "params": params}).encode() + b"\n")
        process.stdin.flush()
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if b"\n" not in pending:
                ready, _, _ = select.select([process.stdout], [], [], deadline - time.monotonic())
                if not ready:
                    break
                chunk = os.read(process.stdout.fileno(), 65536)
                assert chunk, f"server exited during {method}: {process.poll()}"
                pending += chunk
                continue
            line, pending = pending.split(b"\n", 1)
            message = json.loads(line)
            if message.get("id") == sequence:
                assert "error" not in message, message
                return message["result"]
        raise AssertionError(f"timeout during {method}")

    request("initialize", {
        "protocolVersion": "2025-03-26", "capabilities": {},
        "clientInfo": {"name": "upgrade-013-fixture", "version": "1"},
    })
    process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}).encode() + b"\n")
    process.stdin.flush()
    tools = request("tools/list", {})
    assert {"search", "read", "deep_research", "browser", "create_image", "usage"} <= {
        tool["name"] for tool in tools["tools"]
    }


def isolated_env(home):
    env = dict(os.environ, HOME=str(home), USERPROFILE=str(home),
               XDG_CONFIG_HOME=str(home / "config"), FETCHIRA_HOME=str(home), RUST_LOG="off")
    for name in ("FETCHIRA_MASTER_KEY", "FETCHIRA_MASTER_KEY_FILE", "CODEX_HOME"):
        env.pop(name, None)
    return env


def start_current(binary, home):
    env = isolated_env(home)
    process = subprocess.Popen(
        [str(binary), "serve"], cwd=home, env=env, stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0,
    )
    try:
        wait_for_mcp(process)
        return process
    except BaseException:
        process.kill()
        process.wait()
        raise


def stop(process):
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def legacy_queries(db):
    # Exact v0.1.13 read shapes: additive columns/tables must not invalidate them.
    assert db.execute("SELECT used, exhausted FROM usage WHERE label = ? AND period = ?",
                      ("serper-legacy", "lifetime")).fetchone() == (17, 1)
    assert db.execute("SELECT cookies FROM web_session WHERE label = ?",
                      ("grok-legacy",)).fetchone() == ("legacy-cookie-json",)
    assert db.execute("SELECT proxy FROM proxy_assignment WHERE label = ?",
                      ("serper-legacy",)).fetchone() == ("http://legacy-proxy.invalid:8080",)
    assert db.execute("SELECT capability, status FROM route_log WHERE id = 1").fetchone() == ("search", 200)
    assert db.execute("SELECT request, response FROM debug_log WHERE id = 1").fetchone() == ("legacy request", "legacy response")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/fetchira")
    binary = parser.parse_args().binary.resolve()
    assert binary.is_file(), "run `cargo build --locked` first"
    sleep = shutil.which("sleep")
    assert sleep, "the upgrade fixture needs the standard sleep utility"

    with tempfile.TemporaryDirectory(prefix="fetchira-upgrade-013-") as temporary:
        home = Path(temporary)
        config = home / "fetchira.toml"
        config.write_text(V013_CONFIG)
        original_config = config.read_bytes()
        db_path = home / "usage.db"
        db = sqlite3.connect(db_path)
        db.executescript(V013_DDL)
        db.execute("PRAGMA user_version = 1")
        db.execute("INSERT INTO usage VALUES (?, ?, ?, ?, ?)",
                   ("serper", "serper-legacy", "lifetime", 17, 1))
        db.execute("INSERT INTO proxy_assignment VALUES (?, ?)",
                   ("serper-legacy", "http://legacy-proxy.invalid:8080"))
        db.execute("INSERT INTO web_session VALUES (?, ?, ?, ?, ?)",
                   ("grok-legacy", "grok_web", "legacy-cookie-json", "2026-08-03T00:00:00Z", "legacy@example.test"))
        db.execute("INSERT INTO route_log VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                   ("2026-08-03T00:00:00Z", "search", "serper", "serper-legacy", 200, 12,
                    None, None, "", None))
        db.execute("INSERT INTO debug_log VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                   ("2026-08-03T00:00:00Z", "search", "serper", "serper-legacy", 200, 12,
                    "legacy request", "legacy response", None, None))
        db.commit()
        db.close()

        # `instances::running` trusts only a process named fetchira plus this v0.1.13 registry
        # record. A harmless sleep symlink provides that old-process shape without shipping an
        # executable from the historical release in every test environment.
        legacy_name = home / "fetchira"
        legacy_name.symlink_to(sleep)
        peer = subprocess.Popen([str(legacy_name), "30"])
        try:
            (home / "run").mkdir()
            (home / "run" / f"{peer.pid}.json").write_text(json.dumps({
                "pid": peer.pid, "mode": "ui", "version": "0.1.13",
                "host": "claude", "hint": "restart",
            }))
            # The historical UI saves the whole config and would discard an unknown `[remote]`
            # section. New writers must refuse that unsafe overlap before touching the file.
            config.write_bytes(original_config + b"\n[remote]\nendpoint = 'https://example.test/mcp'\napi_key = 'fk_live_fixture'\n")
            before_blocked = config.read_bytes()
            blocked = subprocess.run(
                [str(binary), "add", "serper", "--label", "must-not-save", "--key", "fixture-key"],
                cwd=home, env=isolated_env(home),
                capture_output=True, text=True, timeout=15,
            )
            assert blocked.returncode != 0
            assert "restart older Fetchira dashboards" in blocked.stderr
            assert config.read_bytes() == before_blocked
            config.write_bytes(original_config)
            current = start_current(binary, home)
            stop(current)
            db = sqlite3.connect(db_path)
            assert db.execute("PRAGMA user_version").fetchone() == (1,)
            assert "exhausted_kind" in {row[1] for row in db.execute("PRAGMA table_info(usage)")}
            assert db.execute("SELECT exhausted_kind FROM usage WHERE label = ?", ("serper-legacy",)).fetchone() == (None,)
            legacy_queries(db)
            db.close()
        finally:
            peer.terminate()
            try:
                peer.wait(timeout=5)
            except subprocess.TimeoutExpired:
                peer.kill()
                peer.wait()

        # A dashboard can exist before its best-effort registry write; never allow it to erase
        # new remote settings merely because its entry is absent.
        unknown_dir = home / "unregistered"
        unknown_dir.mkdir()
        unknown_binary = unknown_dir / "fetchira"
        source = unknown_dir / "peer.c"
        source.write_text("#include <unistd.h>\nint main(void) { sleep(30); return 0; }\n")
        subprocess.run(["cc", str(source), "-o", str(unknown_binary)], check=True,
                       capture_output=True, timeout=30)
        unknown = subprocess.Popen([str(unknown_binary), "ui"], cwd=home)
        try:
            config.write_bytes(original_config + b"\n[remote]\nendpoint = 'https://example.test/mcp'\napi_key = 'fk_live_fixture'\n")
            before_blocked = config.read_bytes()
            blocked = subprocess.run(
                [str(binary), "add", "serper", "--label", "must-not-save", "--key", "fixture-key"],
                cwd=home, env=isolated_env(home),
                capture_output=True, text=True, timeout=15,
            )
            assert blocked.returncode != 0 and "restart older Fetchira dashboards" in blocked.stderr, (blocked.returncode, blocked.stderr)
            assert config.read_bytes() == before_blocked
        finally:
            unknown.terminate()
            unknown.wait(timeout=5)
            config.write_bytes(original_config)

        current = start_current(binary, home)
        stop(current)
        db = sqlite3.connect(db_path)
        assert db.execute("PRAGMA user_version").fetchone() == (2,)
        assert {"provider_cooldown", "api_key", "request_log", "request_attempt", "audit_log", "login_challenge"} <= {
            row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type = 'table'")
        }
        legacy_queries(db)
        db.close()
        assert config.read_bytes() == original_config
    print("upgrade 0.1.13 fixture passed: data, legacy peer, schema stamp, config")


if __name__ == "__main__":
    main()
