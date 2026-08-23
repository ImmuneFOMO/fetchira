use std::sync::Arc;

use rmcp::transport::stdio;
use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

use fetchira::cli;
use fetchira::config;
use fetchira::mcp::Fetchira;
use fetchira::router::Router;
use fetchira::usage::Store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let home = cli::home();
    std::fs::create_dir_all(&home).ok();
    // Prefer the home .env, then fall back to one in the working directory.
    dotenvy::from_path(home.join(".env")).ok();
    dotenvy::dotenv().ok();

    // Logs → stderr (stdout is the MCP protocol channel on the serve path). RUST_LOG overrides;
    // default `info`. Initialising here — before the command match — means `ui`/`login` finally
    // surface the browser-capture diagnostics that used to be swallowed.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    use std::io::IsTerminal;
    let mut args = std::env::args().skip(1);
    let cmd = args.next();
    // Passive "new version available" nudge — stderr + TTY only (self-silences under MCP
    // stdio). Skip the serve path (explicit `serve`, or bare with piped stdin) and `update`.
    let serving = matches!(cmd.as_deref(), Some("serve"))
        || (cmd.is_none() && !std::io::stdin().is_terminal());
    if !serving && cmd.as_deref() != Some("update") {
        fetchira::update::nudge_if_stale(&home).await;
    }
    match cmd.as_deref() {
        Some("setup") => return cli::setup(&home).await,
        Some("providers") => {
            cli::providers();
            return Ok(());
        }
        Some("list") | Some("accounts") | Some("usage") => return cli::list(&home).await,
        Some("install") => return cli::install_tools(),
        Some("add") => return cli::add(&home, args).await,
        Some("remove") | Some("rm") => return cli::remove(&home, args.next()).await,
        Some("login") => return cli::login(&home, args.next()).await,
        Some("session") => return cli::session(&home, args).await,
        Some("proxy") => return cli::proxy(&home, args).await,
        Some("priority") => return cli::priority(&home, args),
        Some("ui") => return fetchira::ui::run(&home).await,
        Some("server") => {
            let sub = args.next();
            if sub.as_deref() == Some("password") && args.next().as_deref() == Some("hash") {
                use std::io::{IsTerminal, Read};
                let mut password = String::new();
                if std::io::stdin().is_terminal() {
                    anyhow::bail!("pipe the admin password on stdin; interactive echo is unsafe");
                }
                std::io::stdin().read_to_string(&mut password)?;
                let password = password.trim_end_matches(['\r', '\n']);
                if password.len() < 12 {
                    anyhow::bail!("admin password must be at least 12 characters");
                }
                println!("{}", fetchira::auth::hash_password(password)?);
                return Ok(());
            }
            if sub.as_deref() == Some("healthcheck") {
                let bind = std::env::var("FETCHIRA_HEALTH_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:7879/readyz".into());
                let response = reqwest::Client::new().get(bind).send().await?;
                if !response.status().is_success() {
                    anyhow::bail!("hosted server is not ready: HTTP {}", response.status());
                }
                return Ok(());
            }
            if sub.as_deref() == Some("key") && args.next().as_deref() == Some("create") {
                let id = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing key id"))?;
                let name = args.next().unwrap_or_else(|| id.clone());
                return fetchira::hosted::create_key(&home, id, name).await;
            }
            let bind = std::env::var("FETCHIRA_BIND").unwrap_or_else(|_| "127.0.0.1:7879".into());
            return fetchira::hosted::run(&home, &bind).await;
        }
        Some("serve-http") => {
            let bind = std::env::var("FETCHIRA_BIND").unwrap_or_else(|_| "127.0.0.1:7879".into());
            return fetchira::hosted::run(&home, &bind).await;
        }
        Some("update") => return fetchira::update::run(&home, args).await,
        Some("remote") => match args.next().as_deref() {
            Some("set") => {
                let endpoint = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing endpoint"))?;
                let mut api_key = None;
                while let Some(flag) = args.next() {
                    match flag.as_str() {
                        "--key" => api_key = args.next(),
                        _ => anyhow::bail!("usage: fetchira remote set URL [--key API_KEY]"),
                    }
                }
                return fetchira::remote::set(&home, endpoint, api_key);
            }
            Some("check") => return fetchira::remote::check(&home).await,
            Some("login") => {
                let challenge = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing login challenge"))?;
                let mut file = None;
                let mut browser = None;
                while let Some(flag) = args.next() {
                    match flag.as_str() {
                        "--file" | "-f" => file = args.next(),
                        "--browser" => browser = args.next(),
                        _ => anyhow::bail!(
                            "usage: fetchira remote login CHALLENGE [--browser chrome|firefox] [--file session.json]"
                        ),
                    }
                }
                if browser
                    .as_deref()
                    .is_some_and(|b| !matches!(b, "chrome" | "chromium" | "firefox" | "ff"))
                {
                    anyhow::bail!("--browser must be chrome or firefox");
                }
                return fetchira::remote::login(
                    &home,
                    &challenge,
                    file.as_deref(),
                    browser.as_deref(),
                )
                .await;
            }
            Some("disconnect") => return fetchira::remote::disconnect(&home),
            _ => anyhow::bail!(
                "usage: fetchira remote <set URL [--key API_KEY]|check|login CHALLENGE [--file session.json]|disconnect>"
            ),
        },
        Some("--version") | Some("-V") | Some("version") => {
            println!("fetchira {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("help") | Some("-h") | Some("--help") => {
            cli::help();
            return Ok(());
        }
        Some("serve") => {}
        // Bare `fetchira` from an interactive terminal opens the dashboard; piped
        // (an MCP client) it serves stdio. `FETCHIRA_NO_UI=1` forces serve either way.
        None => {
            if std::io::stdin().is_terminal() && std::env::var("FETCHIRA_NO_UI").is_err() {
                return fetchira::ui::run(&home).await;
            }
        }
        Some(other) => anyhow::bail!("unknown command '{other}' (try `fetchira help`)"),
    }

    // Default: serve the MCP server over stdio. Logs go to stderr (stdout is the MCP channel).
    let cfg_path = home.join("fetchira.toml");
    let mut cfg = config::load(cfg_path.to_str().unwrap_or("fetchira.toml"))
        .map_err(|e| anyhow::anyhow!("{e}. Run `fetchira` in a terminal to set up."))?;
    if cfg.remote.endpoint.is_some() {
        tracing::info!("{}", fetchira::remote::verify(&cfg).await?);
        fetchira::remote::serve_stdio(&cfg).await?;
        return Ok(());
    }
    cfg.db_path = config::resolve_db(&home, &cfg.db_path);
    let store = Store::open(&cfg.db_path).await?;
    let router = Router::build(cfg, store).await?;
    let server = Fetchira::new(Arc::new(router));
    // Registry entry for the schema-aware updater ("which tools still run an old fetchira").
    let _run = fetchira::instances::register(&home, "mcp");

    tracing::info!(
        "fetchira ready; serving MCP over stdio (home: {})",
        home.display()
    );
    server.serve(stdio()).await?.waiting().await?;
    Ok(())
}
