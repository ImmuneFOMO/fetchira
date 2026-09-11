#!/usr/bin/env python3
"""Smoke-test a built stdio server or hosted bridge using an explicit, isolated home."""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", required=True, type=Path)
parser.add_argument("--home", required=True, type=Path)
parser.add_argument("--search-provider", help="Optional real provider call; consumes quota")
parser.add_argument("--read-provider", help="Optional real page read; consumes quota")
parser.add_argument("--check-hosted-file-guard", action="store_true")
parser.add_argument(
    "--check-input-validation",
    action="store_true",
    help="Require the current server to reject invalid tool parameters",
)
args = parser.parse_args()
if not (args.home / "fetchira.toml").is_file():
    parser.error("--home must contain a configured fetchira.toml (an empty file tests no providers)")
env = dict(os.environ, FETCHIRA_HOME=str(args.home.resolve()), HOME=str(args.home.resolve()),
           USERPROFILE=str(args.home.resolve()), XDG_CONFIG_HOME=str(args.home.resolve() / "config"))
env.pop("CODEX_HOME", None)

with tempfile.TemporaryFile() as log:
    process = subprocess.Popen(
        [str(args.binary.resolve()), "serve"],
        cwd=args.home.resolve(), env=env, stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=log, bufsize=0,
    )
    pending = b""
    sequence = 0

    def send(message):
        process.stdin.write(json.dumps(dict(jsonrpc="2.0", **message)).encode() + b"\n")
        process.stdin.flush()

    def request(method, params, expected_error=False):
        global sequence, pending
        sequence += 1
        send(dict(id=sequence, method=method, params=params))
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            if b"\n" not in pending:
                if not select.select([process.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
                    break
                chunk = os.read(process.stdout.fileno(), 65536)
                if not chunk:
                    raise AssertionError(f"server exited during {method}; exit={process.poll()}")
                pending += chunk
                continue
            line, pending = pending.split(b"\n", 1)
            message = json.loads(line)
            if message.get("id") != sequence:
                continue
            if expected_error:
                assert "error" in message, f"expected JSON-RPC rejection during {method}"
                return message["error"]
            assert "error" not in message, f"JSON-RPC error during {method}"
            result = message["result"]
            assert not result.get("isError"), f"tool error during {params.get('name', method)}"
            return result
        raise AssertionError(f"timeout during {method}")

    try:
        request("initialize", {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "fetchira-smoke", "version": "1"},
        })
        send(dict(method="notifications/initialized"))
        tools = request("tools/list", {})
        assert {"search", "read", "deep_research", "browser", "create_image", "usage"} <= {
            tool["name"] for tool in tools["tools"]
        }
        print("PASS initialize + six tools", flush=True)
        if args.check_input_validation:
            for tool, arguments, message in [
                ("search", {"query": "input validation", "topic": "blogs"}, "topic must be"),
                ("read", {"url": "  "}, "exactly one URL"),
                ("deep_research", {"query": "input validation", "depth": "shallow"}, "depth must be"),
            ]:
                error = request(
                    "tools/call",
                    {"name": tool, "arguments": arguments},
                    expected_error=True,
                )
                assert error["code"] == -32602 and message in error["message"], error
            print("PASS invalid parameters are rejected by tool handlers", flush=True)
        if args.check_hosted_file_guard:
            for tool in ["search", "deep_research", "create_image"]:
                arguments = {"file": ["/nonexistent/fetchira-mcp-smoke.txt"]}
                arguments["prompt" if tool == "create_image" else "query"] = "file access check"
                error = request("tools/call", {"name": tool, "arguments": arguments}, expected_error=True)
                assert error["code"] == -32602 and "local stdio" in error["message"]
            print("PASS hosted file inputs rejected for search/research/image", flush=True)
        request("tools/call", {"name": "usage", "arguments": {}})
        print("PASS usage", flush=True)
        for provider, tool, arguments in [
            (args.search_provider, "search", {"query": "Rust programming language official website"}),
            (args.read_provider, "read", {"url": "https://example.com"}),
        ]:
            if provider:
                result = request("tools/call", {
                    "name": tool, "arguments": dict(arguments, provider=provider),
                })
                assert any(item.get("text", "").strip() for item in result.get("content", []))
                print(f"PASS {tool} via {provider}", flush=True)
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
