# Private hosted Fetchira

Hosted mode runs the same router and MCP tools as local stdio, but exposes them at one private HTTPS endpoint. There is no signup, billing or multi-owner tenancy: one administrator owns the server and issues scoped keys to trusted users.

## Docker Compose

Requirements: Docker Engine with Compose v2. Clone the repository on the server and do not commit `.env.hosted` or `secrets/`. The hosted image includes Google Chrome, so the host needs no browser of its own.

Default `docker compose up -d` starts only Fetchira, published on loopback (`127.0.0.1:7879`). Bundled Caddy is the `caddy` profile: it needs a DNS record pointing at the host and free TCP 80/443 plus UDP 443.

```sh
git clone https://github.com/ImmuneFOMO/fetchira.git && cd fetchira
read -rsp 'Fetchira admin password: ' FETCHIRA_PASSWORD; printf '\n'
export FETCHIRA_PASSWORD
install -d -m 700 secrets
head -c 48 /dev/urandom | base64 | tr -d '\n' > secrets/master-key
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

Then, for either track (the master key must already exist so Compose can mount `secrets/`):

```sh
docker compose --env-file .env.hosted -f docker-compose.hosted.yml build fetchira
printf '%s' "$FETCHIRA_PASSWORD" | docker compose --env-file .env.hosted \
  -f docker-compose.hosted.yml run --rm --no-deps --entrypoint fetchira fetchira \
  server password hash > secrets/admin-password
unset FETCHIRA_PASSWORD
chmod 600 .env.hosted secrets/admin-password secrets/master-key
```

Start:

```sh
# Track A
docker compose --env-file .env.hosted -f docker-compose.hosted.yml --profile caddy up -d
curl -fsS "https://$FETCHIRA_HOST/readyz"

# Track B
docker compose --env-file .env.hosted -f docker-compose.hosted.yml up -d
curl -fsS http://127.0.0.1:7879/readyz
```

Copy-paste nginx site (certs from your existing setup):

```nginx
server {
    listen 443 ssl http2;
    server_name fetchira.example.com;

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

`docker-compose.hosted.yml` builds a self-contained image by default. The runtime image installs Google Chrome and configures Fetchira to use it for ChatGPT/Gemini/Grok browser work; the host needs Docker only, not a host browser. If you use a published image, set `FETCHIRA_IMAGE=ghcr.io/your-org/fetchira:tag`, run `docker compose pull fetchira`, and skip the build step above. For an explicit source-built image name, use the override:

```sh
docker compose --env-file .env.hosted \
  -f docker-compose.hosted.yml -f docker-compose.hosted.source.yml \
  build fetchira
docker compose --env-file .env.hosted \
  -f docker-compose.hosted.yml -f docker-compose.hosted.source.yml \
  up -d
```

Add `--profile caddy` to that `up -d` when using bundled Caddy.

The admin password file contains an Argon2id encoded hash, not the password itself. The commands above generate the hash through the same image and do not place the password in shell history. Fetchira also accepts the direct `FETCHIRA_ADMIN_PASSWORD` and `FETCHIRA_MASTER_KEY` environment variables outside Compose; the `_FILE` variants avoid Compose treating Argon2 `$` characters as interpolation syntax.

The image is built reproducibly from the current checkout with `Cargo.lock`; it runs as uid `10001`, read-only except for the named `/data` volume. With `--profile caddy`, Caddy obtains and renews TLS. Open `https://fetchira.example.com/admin` to log in.

Create the first key either in that UI or on the server; its plaintext is printed only once:

```sh
docker compose --env-file .env.hosted -f docker-compose.hosted.yml exec fetchira \
  fetchira server key create owner "Owner laptop"
```

Connect a local Fetchira instance:

```sh
fetchira remote set https://fetchira.example.com/mcp --key 'fk_live_...'
fetchira remote check
```

Coding tools should keep launching local `fetchira` over stdio after `remote set`. Do not paste the hosted URL as a raw MCP endpoint unless the tool speaks Streamable HTTP and bearer auth.

For environment-backed storage, omit `--key` and set `api_key = "env:FETCHIRA_API_KEY"` under `[remote]` in the local `fetchira.toml`. The bridge performs the same authenticated protocol/schema compatibility check as `remote check` before it connects; the local CLI and server may have different app versions when their reported compatibility ranges overlap.

## Bare-metal systemd

Install the release binary into the service-owned state directory, create a locked service account and environment file, then install the included unit:

```sh
sudo useradd --system --home /var/lib/fetchira --shell /usr/sbin/nologin fetchira
sudo install -d -m 700 -o fetchira -g fetchira /var/lib/fetchira
sudo install -o fetchira -g fetchira -m 755 fetchira /var/lib/fetchira/fetchira
sudo install -d -m 750 /etc/fetchira
sudo install -m 600 deploy/hosted.env.example /etc/fetchira/hosted.env
sudo install -m 644 deploy/fetchira.service /etc/systemd/system/fetchira.service
sudo systemctl daemon-reload
sudo systemctl enable --now fetchira
curl -fsS http://127.0.0.1:7879/readyz
```

Keep Fetchira bound to loopback and terminate TLS in Caddy or nginx. The equivalent Caddy site is:

```caddyfile
fetchira.example.com {
    reverse_proxy 127.0.0.1:7879 { flush_interval -1 }
}
```

## Provider login

For web providers, choose Providers → Add provider in the hosted UI. It creates a short-lived, single-use challenge. On the computer with your browser run the displayed command:

```sh
fetchira remote login CHALLENGE
```

The CLI captures the provider session and uploads it over HTTPS; the server encrypts it with `FETCHIRA_MASTER_KEY` and consumes the challenge. Replaying the upload returns `409`/`410`. On a headless machine use the UI paste-session fallback or `fetchira remote login CHALLENGE --file session.json`. Neither the session nor the master key is returned in the API, UI, audit data, or debug logs.

## Backups and restore

SQLite uses WAL. Take a consistent online backup through the update/backup command, or stop the service before copying all database files. For Compose:

```sh
docker compose --env-file .env.hosted -f docker-compose.hosted.yml stop fetchira
docker run --rm -v fetchira_fetchira-data:/data -v "$PWD/backups:/backup" alpine \
  tar czf /backup/fetchira-$(date +%Y%m%d-%H%M%S).tgz -C /data .
docker compose --env-file .env.hosted -f docker-compose.hosted.yml start fetchira
```

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
