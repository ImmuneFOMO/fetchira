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
        Some("ui") => return fetchira::ui::run(&home).await,
        Some("update") => return fetchira::update::run(&home).await,
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
        .map_err(|e| anyhow::anyhow!("{e}. Run `fetchira setup` first."))?;
    cfg.db_path = config::resolve_db(&home, &cfg.db_path);
    let store = Store::open(&cfg.db_path).await?;
    let router = Router::build(cfg, store).await?;
    let server = Fetchira::new(Arc::new(router));

    tracing::info!(
        "fetchira ready; serving MCP over stdio (home: {})",
        home.display()
    );
    server.serve(stdio()).await?.waiting().await?;
    Ok(())
}
