use std::sync::Arc;

use rmcp::transport::stdio;
use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

use fetchira::cli;
use fetchira::config;
use fetchira::mcp::Fetchira;
use fetchira::router::Router;
use fetchira::usage::Store;

fn require_no_args(
    args: &mut impl Iterator<Item = String>,
    usage: &'static str,
) -> anyhow::Result<()> {
    if args.next().is_some() {
        anyhow::bail!(usage);
    }
    Ok(())
}

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
    if !serving
        && !matches!(
            cmd.as_deref(),
            Some("update" | "upgrade" | "--finish-upgrade")
        )
    {
        fetchira::update::nudge_if_stale(&home).await;
    }
    if !matches!(
        cmd.as_deref(),
        Some(
            "install"
                | "setup"
                | "server"
                | "serve-http"
                | "update"
                | "upgrade"
                | "--finish-upgrade"
                | "--version"
                | "-V"
        )
    ) {
        fetchira::update::finish_upgrade_on_start(&home);
    }
    match cmd.as_deref() {
        Some("setup") => {
            require_no_args(&mut args, "usage: fetchira setup")?;
            return cli::setup(&home).await;
        }
        Some("providers") => {
            require_no_args(&mut args, "usage: fetchira providers")?;
            cli::providers();
            return Ok(());
        }
        Some(command @ ("list" | "accounts")) => {
            require_no_args(&mut args, if command == "list" {
                "usage: fetchira list"
            } else {
                "usage: fetchira accounts"
            })?;
            return cli::list(&home).await;
        }
        Some(command @ ("search" | "read" | "deep_research" | "dr" | "browser" | "create_image" | "usage")) => {
            return fetchira::invoke::run(&home, command, args).await;
        }
        Some("install") => {
            match args.next().as_deref() {
                Some("--refresh") => {
                    require_no_args(&mut args, "usage: fetchira install [--refresh]")?;
                    return fetchira::update::refresh_skills(&home);
                }
                Some(_) => anyhow::bail!("usage: fetchira install [--refresh]"),
                None => {}
            }
            return cli::install_tools(&home);
        }
        Some("add") => return cli::add(&home, args).await,
        Some(command @ ("remove" | "rm")) => {
            let label = args.next();
            require_no_args(
                &mut args,
                if command == "rm" {
                    "usage: fetchira rm <label>"
                } else {
                    "usage: fetchira remove <label>"
                },
            )?;
            return cli::remove(&home, label).await;
        }
        Some("login") => {
            let account = args.next();
            require_no_args(&mut args, "usage: fetchira login <provider|label>")?;
            return cli::login(&home, account).await;
        }
        Some("session") => return cli::session(&home, args).await,
        Some("proxy") => return cli::proxy(&home, args).await,
        Some("priority") => return cli::priority(&home, args),
        Some("ui") => {
            require_no_args(&mut args, "usage: fetchira ui")?;
            return fetchira::ui::run(&home).await;
        }
        Some("server") => {
            let sub = args.next();
            if sub.as_deref() == Some("password") && args.next().as_deref() == Some("hash") {
                if args.next().is_some() {
                    anyhow::bail!("usage: fetchira server password hash (password on stdin)");
                }
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
                if args.next().is_some() {
                    anyhow::bail!("usage: fetchira server healthcheck");
                }
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
                let mut name = None;
                let mut accounts_manage = false;
                for arg in args {
                    match arg.as_str() {
                        "--accounts-manage" => accounts_manage = true,
                        _ if !arg.starts_with('-') && name.is_none() => name = Some(arg),
                        _ => anyhow::bail!(
                            "usage: fetchira server key create ID [NAME] [--accounts-manage]"
                        ),
                    }
                }
                if id.starts_with('-') {
                    anyhow::bail!("missing key id before options");
                }
                let name = name.unwrap_or_else(|| id.clone());
                return fetchira::hosted::create_key(&home, id, name, accounts_manage).await;
            }
            if sub.is_some() {
                anyhow::bail!("usage: fetchira server [healthcheck|password hash|key create ID [NAME] [--accounts-manage]]");
            }
            let bind = std::env::var("FETCHIRA_BIND").unwrap_or_else(|_| "127.0.0.1:7879".into());
            return fetchira::hosted::run(&home, &bind).await;
        }
        Some("serve-http") => {
            if args.next().is_some() {
                anyhow::bail!("usage: fetchira serve-http");
            }
            let bind = std::env::var("FETCHIRA_BIND").unwrap_or_else(|_| "127.0.0.1:7879".into());
            return fetchira::hosted::run(&home, &bind).await;
        }
        Some("update" | "upgrade") => return fetchira::update::run(&home, args).await,
        Some("--finish-upgrade") => {
            require_no_args(&mut args, "unexpected upgrade arguments")?;
            return fetchira::update::finish_upgrade(&home);
        }
        Some("remote") => match args.next().as_deref() {
            Some("set") => {
                let endpoint = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing endpoint"))?;
                let mut api_key = None;
                while let Some(flag) = args.next() {
                    match flag.as_str() {
                        "--key" => {
                            api_key = Some(cli::flag_value(&mut args, &flag)?);
                        }
                        _ => anyhow::bail!("usage: fetchira remote set URL [--key API_KEY]"),
                    }
                }
                return fetchira::remote::set(&home, endpoint, api_key);
            }
            Some("check") => {
                if args.next().is_some() {
                    anyhow::bail!("usage: fetchira remote check");
                }
                return fetchira::remote::check(&home).await;
            }
            Some("login") => {
                let challenge = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing login challenge"))?;
                let mut file = None;
                let mut browser = None;
                while let Some(flag) = args.next() {
                    match flag.as_str() {
                        "--file" | "-f" | "--browser" => {
                            let value = cli::flag_value(&mut args, &flag)?;
                            if flag == "--browser" {
                                browser = Some(value);
                            } else {
                                file = Some(value);
                            }
                        }
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
            Some("disconnect") => {
                if args.next().is_some() {
                    anyhow::bail!("usage: fetchira remote disconnect");
                }
                return fetchira::remote::disconnect(&home);
            }
            _ => anyhow::bail!(
                "usage: fetchira remote <set URL [--key API_KEY]|check|login CHALLENGE [--file session.json]|disconnect>"
            ),
        },
        Some("--version") | Some("-V") | Some("version") => {
            require_no_args(&mut args, "usage: fetchira --version")?;
            println!("fetchira {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("help") | Some("-h") | Some("--help") => {
            require_no_args(&mut args, "usage: fetchira help")?;
            cli::help();
            return Ok(());
        }
        Some("serve") => require_no_args(&mut args, "usage: fetchira serve")?,
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
