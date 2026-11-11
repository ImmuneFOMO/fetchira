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
parser.add_argument(
    "--check-live-reload",
    action="store_true",
    help="On a home without fetchira.toml, add and remove an offline exa key in .env and a grok_web "
    "session, and require the running server to follow",
)
args = parser.parse_args()
if not args.home.is_dir():
    parser.error("--home must be an existing directory (an empty fetchira.toml tests no providers)")
env = dict(os.environ, FETCHIRA_HOME=str(args.home.resolve()), HOME=str(args.home.resolve()),
           USERPROFILE=str(args.home.resolve()), XDG_CONFIG_HOME=str(args.home.resolve() / "config"))
env.pop("CODEX_HOME", None)
if args.check_live_reload:
    env.pop("EXA_API_KEY", None)
    assert not (args.home / ".env").exists(), "--check-live-reload needs a home without .env"
    (args.home / ".env").write_text("")

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

    def receive(deadline, during):
        global pending
        while b"\n" not in pending:
            if not select.select([process.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
                raise AssertionError(f"timeout during {during}")
            chunk = os.read(process.stdout.fileno(), 65536)
            if not chunk:
                raise AssertionError(f"server exited during {during}; exit={process.poll()}")
            pending += chunk
        line, pending = pending.split(b"\n", 1)
        return json.loads(line)

    def request(method, params, expected_error=False):
        global sequence
        sequence += 1
        send(dict(id=sequence, method=method, params=params))
        deadline = time.monotonic() + 180
        while (message := receive(deadline, method)).get("id") != sequence:
            pass
        if expected_error:
            assert "error" in message, f"expected JSON-RPC rejection during {method}"
            return message["error"]
        assert "error" not in message, f"JSON-RPC error during {method}"
        result = message["result"]
        assert not result.get("isError"), f"tool error during {params.get('name', method)}"
        return result

    def tool_names():
        tools = request("tools/list", {})["tools"]
        search = next(tool for tool in tools if tool["name"] == "search")
        web = {"gemini_web", "grok_web", "chatgpt_web"} & set(
            search["inputSchema"]["$defs"]["ProviderKind"]["enum"]
        )
        return tools, {tool["name"] for tool in tools}, web

    try:
        info = request("initialize", {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "fetchira-smoke", "version": "1"},
        })
        send(dict(method="notifications/initialized"))
        tools, names, web = tool_names()
        assert names == {"search", "read", "deep_research", "browser", "usage"} | (
            {"create_image"} if web else set()
        ), names
        assert all(
            tool.get("title") and "readOnlyHint" in tool.get("annotations", {})
            for tool in tools
        )
        print(f"PASS initialize + {len(names)} tools", flush=True)
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
        if args.check_live_reload:
            assert info["capabilities"]["tools"].get("listChanged"), info["capabilities"]
            assert not web, "--check-live-reload needs a home without web sessions"
            assert not (args.home / "fetchira.toml").exists(), "--check-live-reload needs a home without fetchira.toml"

            def wait_for_exa(present):
                deadline = time.monotonic() + 10
                while True:
                    text = request("tools/call", {"name": "usage", "arguments": {}})["content"][0]["text"]
                    line = next(line for line in text.splitlines() if line.startswith("api keys:"))
                    keys = {entry.split()[0] for entry in line[len("api keys:"):].split(" · ") if entry.strip()}
                    if ("exa" in keys) == present:
                        return
                    assert time.monotonic() < deadline, f"exa {'missing' if present else 'still listed'}: {keys}"
                    time.sleep(0.2)

            wait_for_exa(False)
            (args.home / ".env").write_text("EXA_API_KEY=fetchira-smoke-fake\n")
            wait_for_exa(True)
            (args.home / ".env").write_text("")
            wait_for_exa(False)
            print("PASS a key added to and removed from the home .env applies without a restart", flush=True)
            with open(args.home / "fetchira.toml", "a") as config:
                config.write('\n[[account]]\nprovider = "grok_web"\nlabel = "grok-reload"\n')
            session = args.home / "grok-reload.json"
            session.write_text(json.dumps([{"name": "sso", "value": "offline", "domain": ".grok.com"}]))
            subprocess.run(
                [str(args.binary.resolve()), "session", "grok-reload", "--file", str(session)],
                cwd=args.home.resolve(), env=env, check=True, capture_output=True,
            )
            deadline = time.monotonic() + 10
            while receive(deadline, "tools/list_changed").get("method") != "notifications/tools/list_changed":
                pass
            _, names, web = tool_names()
            assert "create_image" in names and "grok_web" in web, (names, web)
            print("PASS tool list follows a new web session without a restart", flush=True)
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
