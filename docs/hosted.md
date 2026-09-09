# Private hosted Fetchira

Hosted mode runs the same router and MCP tools as local stdio, but exposes them at one private HTTPS endpoint. There is no signup, billing or multi-owner tenancy: one administrator owns the server and issues scoped keys to trusted users.

## Docker Compose

Requirements: Docker Engine with Compose v2 and `openssl`. Clone the repository on the server and do not commit `.env.hosted` or `secrets/`. The hosted image includes Google Chrome, so the host needs no browser of its own.

Default `docker compose up -d` starts only Fetchira, published on loopback (`127.0.0.1:7879`). Bundled Caddy is the `caddy` profile: it needs a DNS record pointing at the host and free TCP 80/443 plus UDP 443.

```sh
git clone https://github.com/ImmuneFOMO/fetchira.git && cd fetchira
install -d -m 700 secrets
if [ ! -e secrets/master-key ]; then
    (umask 077; set -C; openssl rand -base64 32 > secrets/master-key) 2>/dev/null || [ -s secrets/master-key ]
fi
touch secrets/admin-password
```

The master-key check preserves an existing key; never regenerate it over the same path. An empty `admin-password` file selects first-visit setup and `touch` preserves a pre-seeded hash on later runs. Do not use `: > secrets/admin-password` here: it truncates a pre-seeded hash.

To pre-seed the admin password instead of using first-visit setup, generate its Argon2id hash now:

```sh
printf '%s\n' 'your-password' | fetchira server password hash > secrets/admin-password
```

Write `.env.hosted` (track A or B), then build once.

### Track A — fresh VPS, bundled Caddy

Needs a DNS record and inbound TCP 80/443 plus UDP 443.

```sh
export FETCHIRA_HOST=fetchira.example.com
printf '%s\n' "FETCHIRA_HOST=$FETCHIRA_HOST" > .env.hosted
```

### Track B — existing nginx

Do not start Caddy. Fetchira stays on loopback; nginx terminates TLS.

```sh
export FETCHIRA_HOST=fetchira.example.com
cat > .env.hosted <<EOF
FETCHIRA_HOST=$FETCHIRA_HOST
FETCHIRA_PORT=127.0.0.1:7879
EOF
```

Then, for either track (the admin-password file and master key must already exist so Compose can mount `secrets/`):

```sh
docker compose --env-file .env.hosted -f docker-compose.hosted.yml build fetchira
chmod 600 .env.hosted
```

On Linux, Docker Compose implements file-based secrets as bind mounts and ignores the service `uid`, `gid`, and `mode` fields. Keep the parent directory private and let the container read the mounted files:

```sh
chmod 700 secrets
chmod 644 secrets/admin-password secrets/master-key
```

The `secrets/` directory mode `700` keeps other host users from reaching the files; mode `644` is needed because the bind-mounted file keeps host ownership while the service runs as uid `10001`. Docker Desktop also maps these mounts correctly. A Compose warning that `uid`, `gid` and `mode` are ignored is expected on the file-secret path.

Start Fetchira by itself first. This keeps the admin setup private while you set the password:

```sh
docker compose --env-file .env.hosted -f docker-compose.hosted.yml up -d
curl -fsS http://127.0.0.1:7879/readyz
```

From your laptop, run `ssh -N -L 7879:127.0.0.1:7879 USER@HOST`, open `http://127.0.0.1:7879/admin`, and set an admin password of at least 12 characters. Finish this first-visit setup before enabling Caddy or nginx and exposing the hostname.

For Track A, start the bundled TLS proxy after the password is set:

```sh
docker compose --env-file .env.hosted -f docker-compose.hosted.yml --profile caddy up -d
curl -fsS "https://$FETCHIRA_HOST/readyz"
```

For Track B, configure nginx below, reload it after the password is set, then verify:

```sh
curl -fsS "https://$FETCHIRA_HOST/readyz"
```

Copy-paste nginx site and replace the certificate paths if your existing TLS setup uses different files:

```nginx
server {
    listen 443 ssl http2;
    server_name fetchira.example.com;
    ssl_certificate /etc/letsencrypt/live/fetchira.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/fetchira.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:7879;
        proxy_http_version 1.1;
        proxy_buffering off;
        proxy_read_timeout 300s;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Then:

```sh
curl -fsS "https://$FETCHIRA_HOST/readyz"
```

`docker-compose.hosted.yml` builds a self-contained image by default. The runtime image installs Google Chrome for hosted ChatGPT requests and live-limit checks; uploaded Gemini/Grok sessions can run over HTTP, and the host needs Docker only, not a host browser. If you use a published image, set `FETCHIRA_IMAGE=ghcr.io/your-org/fetchira:tag`, run `docker compose pull fetchira`, and skip the build step above. For an explicit source-built image name, use the override:

```sh
docker compose --env-file .env.hosted \
  -f docker-compose.hosted.yml -f docker-compose.hosted.source.yml \
  build fetchira
docker compose --env-file .env.hosted \
  -f docker-compose.hosted.yml -f docker-compose.hosted.source.yml \
  up -d
```

Add `--profile caddy` to that `up -d` when using bundled Caddy.

Leave `secrets/admin-password` empty for first-visit setup: open `/admin` through the SSH tunnel and set the password in the browser. The server stores the Argon2id hash on the `/data` volume; the plaintext never sits in Compose files, shell history, or agent logs. The pre-seeded hash command is above; a TTY is refused. Never put the login password itself in `FETCHIRA_ADMIN_PASSWORD` — only that Argon2id hash (or leave the env unset). Outside Compose, `FETCHIRA_ADMIN_PASSWORD` and `FETCHIRA_MASTER_KEY` work as env vars; the `_FILE` variants avoid Compose treating Argon2 `$` characters as interpolation syntax.

The image is built from the current checkout with `Cargo.lock`; it runs as uid `10001`, read-only except for the named `/data` volume and temporary files in `/tmp`. With `--profile caddy`, Caddy obtains and renews TLS. Open `https://fetchira.example.com/admin` after the private first-visit setup to log in.

Create the first key either in that UI or on the server; its plaintext is printed only once. CLI `server key create` defaults to `mcp` + `usage:read`. Add `--accounts-manage` only for a trusted owner: it enables `remote login` and all account mutations, including deleting accounts and replacing credentials. The Keys UI defaults to `mcp` + `usage:read`; select `accounts:manage` only if that laptop will upload web sessions.

```sh
docker compose --env-file .env.hosted -f docker-compose.hosted.yml exec fetchira \
  fetchira server key create laptop "Laptop"
```

If this key must run `fetchira remote login`, create it with `--accounts-manage` instead. If an older installed binary rejects that option, use the Keys UI; the option is in the current checkout and the next release.

Connect a local Fetchira instance:

```sh
fetchira remote set https://fetchira.example.com/mcp --key 'fk_live_...'
fetchira remote check
```

Coding tools should keep launching local `fetchira` over stdio after `remote set`. Do not paste the hosted URL as a raw MCP endpoint unless the tool speaks Streamable HTTP and bearer auth.

For environment-backed storage, omit `--key` and set `api_key = "env:FETCHIRA_API_KEY"` under `[remote]` in the local `fetchira.toml`. The bridge performs the same authenticated protocol/schema compatibility check as `remote check` before it connects; the local CLI and server may have different app versions when their reported compatibility ranges overlap.

Add API-key providers from `/admin` (preferred). As a CLI fallback, configure them on the server with `fetchira add PROVIDER --key 'KEY'` while the server is stopped, then start or restart the hosted process. Keep `--key`; omitting it starts an interactive prompt. Add web-session providers from the admin UI and use its challenge flow below.

The hosted bridge does not upload laptop file paths, so file attachments and file Q&A require local mode. Hosted image outputs are returned to the local bridge and saved on the laptop.

## Bare-metal systemd

Install the release binary into the service-owned state directory, create a locked service account and environment file, then install the included unit. Keep the master key in a persistent file; leave `FETCHIRA_ADMIN_PASSWORD` unset for first-visit setup at `/admin`.

```sh
sudo useradd --system --user-group --home /var/lib/fetchira --shell /usr/sbin/nologin fetchira
sudo install -d -m 700 -o fetchira -g fetchira /var/lib/fetchira
sudo install -o fetchira -g fetchira -m 755 fetchira /var/lib/fetchira/fetchira
sudo install -d -m 750 -o root -g fetchira /etc/fetchira
sudo install -m 600 deploy/hosted.env.example /etc/fetchira/hosted.env
if ! sudo test -e /etc/fetchira/master-key; then
  openssl rand -base64 32 | sudo tee /etc/fetchira/master-key > /dev/null
fi
sudo chown fetchira:fetchira /etc/fetchira/master-key
sudo chmod 600 /etc/fetchira/master-key
printf '%s\n' 'FETCHIRA_MASTER_KEY_FILE=/etc/fetchira/master-key' | sudo tee -a /etc/fetchira/hosted.env > /dev/null
sudo chmod 600 /etc/fetchira/hosted.env
sudo install -m 644 deploy/fetchira.service /etc/systemd/system/fetchira.service
sudo systemctl daemon-reload
sudo systemctl enable --now fetchira
curl -fsS http://127.0.0.1:7879/readyz
sudo -u fetchira env FETCHIRA_HOME=/var/lib/fetchira \
  FETCHIRA_MASTER_KEY_FILE=/etc/fetchira/master-key \
  /var/lib/fetchira/fetchira server key create laptop "Laptop"
# Add --accounts-manage only to a trusted key that uploads web sessions:
# sudo -u fetchira env FETCHIRA_HOME=/var/lib/fetchira FETCHIRA_MASTER_KEY_FILE=/etc/fetchira/master-key \
#   /var/lib/fetchira/fetchira server key create owner "Owner laptop" --accounts-manage
```

The key-generation check preserves an existing master key on reruns. Keep Fetchira bound to loopback while you set the first admin password through an SSH tunnel (`ssh -N -L 7879:127.0.0.1:7879 USER@HOST`), then terminate TLS in Caddy or nginx before publishing the hostname. API-key providers and uploaded Gemini/Grok web sessions work without a host browser. ChatGPT requests and live limits always need Chrome/Chromium on the machine running the hosted process; the Compose image includes Chrome. The equivalent Caddy site is:

```caddyfile
fetchira.example.com {
    reverse_proxy 127.0.0.1:7879 {
        flush_interval -1
    }
}
```

## Provider login

For web providers, choose Providers → Add provider in the hosted UI. It creates a short-lived, single-use challenge. On the computer with your browser, configure `fetchira remote set https://fetchira.example.com/mcp --key 'fk_live_...'` first, then run the displayed command:

```sh
fetchira remote login CHALLENGE
```

The CLI captures the provider session and uploads it over HTTPS; the server encrypts it with `FETCHIRA_MASTER_KEY` and consumes the challenge. The laptop key needs `accounts:manage` (create the key with `fetchira server key create ID [NAME] --accounts-manage`). Replaying the upload returns `409`/`410`. On a headless machine use the UI paste-session fallback or `fetchira remote login CHALLENGE --file session.json`; ChatGPT still needs Chrome/Chromium on the hosted machine when it handles requests. Neither the session nor the master key is returned in the API, UI, audit data, or debug logs.

## Backups and restore

SQLite uses WAL. Take a consistent online backup through the update/backup command, or stop the service before copying all database files. For Compose:

```sh
mkdir -p backups
docker compose --env-file .env.hosted -f docker-compose.hosted.yml stop fetchira
docker run --rm -v fetchira_fetchira-data:/data -v "$PWD/backups:/backup" alpine \
  tar czf /backup/fetchira-$(date +%Y%m%d-%H%M%S).tgz -C /data .
docker compose --env-file .env.hosted -f docker-compose.hosted.yml start fetchira
```

The archive includes `fetchira.toml`, `usage.db` and its WAL files, and the `/data/admin-password` hash created during first-visit setup. If the Compose project name is not `fetchira`, replace `fetchira_fetchira-data` with the volume shown by `docker volume ls`.

To restore, stop Fetchira, archive the current volume, extract the chosen backup into `/data`, keep ownership uid/gid `10001`, start Fetchira, then require both `/healthz` and `/readyz`. Never restore only `usage.db` while its `-wal` file is live.

Back up `secrets/` separately in a password manager. Losing the master key makes encrypted provider credentials unrecoverable; rotating it requires an application-assisted re-encryption, not replacing the file alone.

## Updates and rollback

The admin update job checks release metadata and checksums, drains active MCP calls, snapshots SQLite, applies migrations, and re-executes the service with the new binary. `Update when idle` waits for the server to become idle. The database snapshot is retained under `backups/` for operator-led restore if needed; the local CLI updates independently with `fetchira update`.

For source-built Compose deployments, the explicit operator fallback is:

```sh
git fetch --tags
git checkout vX.Y.Z
docker compose --env-file .env.hosted -f docker-compose.hosted.yml -f docker-compose.hosted.source.yml build --pull fetchira
docker compose --env-file .env.hosted -f docker-compose.hosted.yml -f docker-compose.hosted.source.yml up -d fetchira
docker compose --env-file .env.hosted -f docker-compose.hosted.yml -f docker-compose.hosted.source.yml exec fetchira fetchira server healthcheck
```

Pin an immutable release tag. Do not deploy a moving branch to a private server you depend on.

If a CDN such as Cloudflare sits in front of the server, bypass caching for `/admin`, `/admin/*`, and `/mcp`, and preserve the origin's `Cache-Control` headers. Cached admin JavaScript can otherwise show an older UI after a deployment; purge that cache and hard-reload the browser after changing the rule.

## Control-plane contract

Key inventory, usage, audit and provider-read endpoints require the admin session. Mutations accept either that session plus the `fetchira_csrf` cookie echoed in `X-CSRF-Token`, or a non-revoked API key carrying the route's scope (`accounts:manage` for account/login changes, `server:update` for updates). All control-plane responses are JSON and never contain credential fields except the one-time plaintext returned by key creation.

| Method | Route | Purpose |
|---|---|---|
| `GET` | `/admin` | Embedded hosted UI |
| `POST` | `/admin/login` | Password login; sets separate admin and CSRF cookies |
| `GET/POST` | `/admin/keys` | Redacted key inventory / one-time key creation |
| `POST` | `/admin/keys/{id}/revoke` | Revoke a key |
| `GET` | `/admin/usage` | Filtered request ledger |
| `GET` | `/admin/usage/{id}/attempts` | Provider/failover attempts for one request |
| `GET` | `/admin/providers` | Redacted provider readiness and identity |
| `POST` | `/admin/providers/key` | Add an API-key provider account |
| `POST` | `/admin/providers/session` | Encrypt and attach pasted credential/session |
| `POST` | `/admin/login-challenges` | Create browser-login challenge |
| `GET` | `/admin/login-challenges/{challenge}` | Poll challenge state; never returns session data |
| `GET/POST` | `/admin/update` | Update job status / start `now` or `idle` job |
| `GET` | `/admin/audit` | Administrative audit ledger |
| `GET` | `/version` | Server/protocol/schema compatibility (public, no secrets) |

The challenge upload used by the local CLI is `GET/POST /login-challenges/{challenge}` with its remote API key; upload JSON contains `session`, is encrypted before persistence, and the challenge is atomically consumed. A friend key's `GET /usage` response is always constrained to that key id. Retention and exports apply the same redaction policy as the UI.

## Verification

Run the local contract smoke test against an already-running server, then the full Rust suite:

```sh
FETCHIRA_URL=https://fetchira.example.com FETCHIRA_ADMIN_PASSWORD_PLAIN='…' \
  ./deploy/hosted-smoke.sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
docker compose --env-file .env.hosted -f docker-compose.hosted.yml config
```

The smoke test performs no provider calls and creates no keys: it verifies health/version, unauthenticated rejection, login cookie separation, CSRF enforcement and authenticated inventory access.
