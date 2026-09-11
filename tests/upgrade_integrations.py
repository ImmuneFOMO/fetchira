#!/usr/bin/env python3
"""Check automatic integration migration on local startup using an isolated binary/home."""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import tomllib
from urllib.request import Request, ProxyHandler, build_opener

from upgrade_013 import isolated_env, wait_for_mcp, stop


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'target/debug/fetchira')
    source = parser.parse_args().binary.resolve()
    for mode in ('serve', 'bare', 'ui', 'finish'):
        with tempfile.TemporaryDirectory(prefix='fetchira-auto-upgrade-') as temporary:
            home = Path(temporary).resolve()
            data = home / 'data'
            data.mkdir()
            config = data / 'fetchira.toml'
            config.write_text('# existing user configuration\ndb_path = "usage.db"\n')
            before_config = config.read_bytes()
            # Model Homebrew removing the old keg while retaining a stable launcher.
            prefix = home / 'brew'
            binary = prefix / 'Cellar/fetchira/current/bin/fetchira'
            binary.parent.mkdir(parents=True)
            shutil.copy2(source, binary)
            stable = prefix / 'bin/fetchira'
            stable.parent.mkdir()
            stable.symlink_to(binary)
            old = prefix / 'Cellar/fetchira/0.1.13/bin/fetchira'
            codex = home / '.codex'
            skill = codex / 'skills/fetchira-mcp'
            skill.mkdir(parents=True)
            (skill / 'SKILL.md').write_text('old instructions')
            (skill / 'custom.txt').write_text('user content to preserve')
            registration = codex / 'config.toml'
            registration.write_text(f'''model = "user-choice"
[mcp_servers.fetchira]
command = "{old}"
args = ["serve"]
[mcp_servers.fetchira.env]
FETCHIRA_HOME = "{data}"
CUSTOM = "keep"
[mcp_servers.other]
command = "/user/other"
''')
            before_registration = registration.read_text()
            env = isolated_env(home)
            env.update(FETCHIRA_HOME=str(data), FETCHIRA_NO_OPEN='1', PATH=str(stable.parent) + os.pathsep + env['PATH'])
            version = subprocess.run([str(stable), '--version'], env=env, cwd=home,
                                     capture_output=True, text=True, check=True).stdout.split()[-1]
            assert registration.read_text() == before_registration, '--version must not migrate integrations'
            assert (skill / 'SKILL.md').read_text() == 'old instructions'
            processes = []
            try:
                if mode in ('serve', 'bare'):
                    # Two agents start together; both must get a clean MCP handshake.
                    for _ in range(2):
                        processes.append(subprocess.Popen([str(stable)] + (['serve'] if mode == 'serve' else []),
                            env=env, cwd=home, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, bufsize=0))
                    for process in processes:
                        wait_for_mcp(process)
                elif mode == 'ui':
                    with socket.socket() as sock:
                        sock.bind(('127.0.0.1', 0))
                        port = sock.getsockname()[1]
                    env.update(FETCHIRA_UI_PORT=str(port), FETCHIRA_UI_TOKEN='migration-test')
                    processes.append(subprocess.Popen([str(stable), 'ui'], env=env, cwd=home,
                        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE))
                    request = Request(f'http://127.0.0.1:{port}/api/install/targets', headers={'x-fetchira-token': 'migration-test'})
                    opener = build_opener(ProxyHandler({}))
                    deadline = time.monotonic() + 15
                    while True:
                        try:
                            with opener.open(request, timeout=1) as response:
                                targets = json.load(response)
                            assert not targets['upgradePending']
                            assert targets['outdatedSkills'] == []
                            break
                        except OSError:
                            if time.monotonic() >= deadline:
                                raise
                            time.sleep(0.1)
                else:
                    subprocess.run([str(stable), '--finish-upgrade'], env=env, cwd=home,
                                   capture_output=True, check=True, timeout=15)
                assert (data / 'agent-upgrade-complete').read_text() == version
                assert tomllib.loads(registration.read_text()) == tomllib.loads(before_registration.replace(str(old), str(stable)))
                assert config.read_bytes() == before_config
                current_skill = home / '.agents/skills/fetchira-mcp'
                assert (current_skill / 'references.md').is_file()
                assert 'old instructions' not in (current_skill / 'SKILL.md').read_text()
                assert not (home / '.agents/skills/fetchira-cli').exists(), 'MCP choice must not change'
                preserved = list(home.rglob('custom.txt'))
                assert len(preserved) == 1 and preserved[0].read_text() == 'user content to preserve'
                marker_time = (data / 'agent-upgrade-complete').stat().st_mtime_ns
                subprocess.run([str(stable), '--finish-upgrade'], env=env, cwd=home,
                               capture_output=True, check=True, timeout=15)
                assert (data / 'agent-upgrade-complete').stat().st_mtime_ns == marker_time
                assert len(list(home.rglob('custom.txt'))) == 1
                print(f'PASS automatic {mode}: skill migration, Homebrew launcher repair, backups, choices, idempotency', flush=True)
            finally:
                for process in processes:
                    stop(process)


if __name__ == '__main__':
    main()
