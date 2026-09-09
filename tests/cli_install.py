#!/usr/bin/env python3
"""Offline install API check. Run after cargo build: python3 tests/cli_install.py"""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request, ProxyHandler, build_opener


def main():
    binary = Path(__file__).resolve().parents[1] / 'target/debug/fetchira'
    with tempfile.TemporaryDirectory(prefix='fetchira-install-api-') as temp:
        home = Path(temp)
        for parent in ('.claude', '.cursor', '.codex', '.gemini', 'config', 'data'):
            (home / parent).mkdir()
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        url = f'http://127.0.0.1:{port}'
        env = dict(os.environ, HOME=temp, USERPROFILE=temp,
                   XDG_CONFIG_HOME=str(home / 'config'), FETCHIRA_HOME=str(home / 'data'),
                   FETCHIRA_UI_PORT=str(port), FETCHIRA_UI_TOKEN='install-test',
                   FETCHIRA_NO_OPEN='1', RUST_LOG='off',
                   HTTP_PROXY='http://127.0.0.1:9', HTTPS_PROXY='http://127.0.0.1:9',
                   ALL_PROXY='http://127.0.0.1:9', NO_PROXY='127.0.0.1,localhost')
        env.pop('CODEX_HOME', None)
        opener = build_opener(ProxyHandler({}))

        def request(path, body=None, token='install-test'):
            data = json.dumps(body).encode() if body is not None else None
            req = Request(url + path, data=data, headers={
                'x-fetchira-token': token, 'Origin': url, 'Content-Type': 'application/json',
            })
            try:
                with opener.open(req, timeout=3) as response:
                    return response.status, json.load(response)
            except HTTPError as error:
                return error.code, error.read().decode()

        with (home / 'server.log').open('w+') as log:
            process = subprocess.Popen([str(binary), 'ui'], env=env, cwd=temp,
                                       stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 10
                while True:
                    try:
                        status, initial = request('/api/install/targets')
                        assert status == 200
                        break
                    except URLError:
                        assert process.poll() is None, 'dashboard exited before startup'
                        assert time.monotonic() < deadline, 'dashboard startup timed out'
                        time.sleep(0.05)
                assert initial['skills'] == []
                assert request('/api/install', {'targets': [], 'skill': 'cli'}, token='wrong')[0] == 401
                # Invalid skill must be rejected before even a selected MCP target writes.
                assert request('/api/install', {'targets': ['Codex CLI'], 'skill': 'bad'})[0] == 400
                assert not (home / '.codex/config.toml').exists()
                status, result = request('/api/install', {'targets': []})
                assert status == 200 and result['results'] == []
                assert not (home / '.codex/skills').exists()
                status, result = request('/api/install', {'targets': [], 'skill': 'cli'})
                assert status == 200 and all(r['ok'] for r in result['results']), result
                for parent in ('.claude', '.cursor', '.agents'):
                    folder = home / parent / 'skills/fetchira-cli'
                    assert 'name: fetchira-cli' in (folder / 'SKILL.md').read_text()
                    assert '## Sessions' in (folder / 'references.md').read_text()
                assert not (home / '.codex/skills/fetchira-cli').exists()
                assert not (home / '.gemini/skills/fetchira-cli').exists()
                assert not (home / '.codex/config.toml').exists()
                status, detection = request('/api/install/targets')
                assert status == 200 and detection['skills'] == ['cli']
                custom = home / '.agents/skills/fetchira-cli/custom.md'
                custom.write_text('keep this user file')
                status, result = request('/api/install', {'targets': [], 'skill': 'both'})
                assert status == 200 and all(r['ok'] for r in result['results']), result
                assert not (home / '.agents/skills/fetchira-cli').exists()
                assert (home / '.agents/skills/fetchira/SKILL.md').is_file()
                assert any(p.read_text() == 'keep this user file' for p in (home / '.agents').rglob('custom.md'))
                assert request('/api/install', {'targets': [], 'skill': 'skip'})[0] == 200
                assert request('/api/install/targets')[1]['skills'] == ['both']
                print('install API passed: auth, validation, compatibility, CLI-only, switch, skip, detection')
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    main()
