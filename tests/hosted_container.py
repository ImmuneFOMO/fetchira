#!/usr/bin/env python3
"""Check an existing Docker image without provider calls: python3 tests/hosted_container.py IMAGE."""
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, **kwargs).stdout.strip()


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else 'fetchira:smoke'
    name = 'fetchira-smoke-' + secrets.token_hex(6)
    opener = build_opener(ProxyHandler({}))
    with tempfile.TemporaryDirectory(prefix='fetchira-container-') as temp:
        password = secrets.token_urlsafe(24)
        password_hash = run('docker', 'run', '--rm', '-i', '--entrypoint',
                            '/usr/local/bin/fetchira', image, 'server', 'password', 'hash',
                            input=password + '\n')
        env_file = Path(temp) / 'env'
        env_file.write_text('FETCHIRA_MASTER_KEY=' + secrets.token_urlsafe(32) + '\n'
                            'FETCHIRA_ADMIN_PASSWORD=' + password_hash + '\n')
        env_file.chmod(0o600)
        try:
            run('docker', 'run', '-d', '--name', name, '--init', '--read-only',
                '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges:true',
                '--tmpfs', '/tmp:rw,size=512m,mode=1777', '--env-file', str(env_file),
                '-p', '127.0.0.1::7879', image)
            port = run('docker', 'port', name, '7879').rsplit(':', 1)[1]
            url = 'http://127.0.0.1:' + port
            deadline = time.monotonic() + 30
            while True:
                try:
                    with opener.open(url + '/readyz', timeout=1) as response:
                        assert response.status == 200
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise RuntimeError('container did not become ready')
                    time.sleep(0.2)
            run('docker', 'exec', name, '/data/fetchira', 'server', 'healthcheck')
            smoke = Path(__file__).resolve().parents[1] / 'deploy/hosted-smoke.sh'
            print(run('sh', str(smoke), env=dict(os.environ, FETCHIRA_URL=url,
                                                FETCHIRA_ADMIN_PASSWORD_PLAIN=password)))
            key_output = run('docker', 'exec', name, '/data/fetchira', 'server', 'key',
                             'create', 'smoke', 'Smoke', '--accounts-manage')
            key = next(value for value in key_output.split() if value.startswith('fk_live_'))

            def api(path, body, status=200):
                request = Request(url + path, data=json.dumps(body).encode(), headers={
                    'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json',
                })
                try:
                    with opener.open(request, timeout=5) as response:
                        assert response.status == status
                        return json.load(response)
                except HTTPError as error:
                    assert error.code == status, f'{path}: expected {status}, got {error.code}'

            challenge = api('/admin/login-challenges', {'provider': 'gemini_web', 'label': 'custom-gemini'})
            assert challenge['label'] == 'custom-gemini'
            config = run('docker', 'exec', name, 'cat', '/data/fetchira.toml')
            assert 'label = "custom-gemini"' in config and 'provider = "gemini_web"' in config
            api('/admin/login-challenges', {'provider': 'grok_web', 'label': 'custom-gemini'}, 400)
            upload = {'session': json.dumps({'cookies': [{
                'name': '__Secure-1PSID', 'value': 'offline-fixture', 'domain': '.google.com', 'path': '/',
            }]}), 'identity': 'fixture', 'plan': 'test', 'limits': {}}
            upload_path = '/login-challenges/' + challenge['challenge']
            assert api(upload_path, upload)['ok']
            api(upload_path, upload, 410)
            print('PASS custom-label login, provider conflict and single-use session upload')
            run('docker', 'exec', name, 'sh', '-c',
                'google-chrome-stable --no-sandbox --disable-dev-shm-usage --disable-gpu '
                '--no-first-run --no-default-browser-check --user-data-dir=/tmp/chrome-smoke '
                '--remote-debugging-port=9222 about:blank >/tmp/chrome-smoke.log 2>&1 &')
            version = run('docker', 'exec', name, 'sh', '-c',
                          'for attempt in 1 2 3 4 5; do '
                          'wget -qO- http://127.0.0.1:9222/json/version && exit 0; sleep 1; '
                          'done; cat /tmp/chrome-smoke.log >&2; exit 1')
            version = json.loads(version)
            assert version.get('webSocketDebuggerUrl'), 'Chrome CDP is unavailable'
            print('PASS headful Chrome on Xvfb:', version['Browser'])
        except Exception:
            logs = subprocess.run(['docker', 'logs', name], text=True, capture_output=True)
            print(logs.stdout + logs.stderr, file=sys.stderr)
            raise
        finally:
            subprocess.run(['docker', 'rm', '-f', '-v', name], capture_output=True)


if __name__ == '__main__':
    main()
