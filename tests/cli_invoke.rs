use std::process::{Command, Output};

#[test]
fn one_shot_contract_without_live_providers() {
    let home = std::env::temp_dir().join(format!(
        "fetchira_invoke_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).unwrap();
    let run = |args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_fetchira"))
            .args(args)
            .env("FETCHIRA_HOME", &home)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env_remove("CODEX_HOME")
            .env("RUST_LOG", "off")
            .current_dir(&home)
            .output()
            .unwrap()
    };
    for args in [
        vec!["search"],
        vec!["search", ""],
        vec!["create_image", "   "],
        vec!["read"],
        vec!["browser", "a", "b"],
        vec!["search", "q", "--max", "bad"],
        vec!["read", "url", "--file", "file"],
        vec!["usage", "no-provider"],
        vec!["dr", "q", "--depth", "bad"],
        vec!["create_image"],
        vec!["search", "q", "--unknown"],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("usage: fetchira"),
            "{args:?}"
        );
        assert!(
            !home.join("usage.db").exists(),
            "argument errors must precede router setup"
        );
    }
    for command in [
        "search",
        "read",
        "deep_research",
        "dr",
        "browser",
        "create_image",
        "usage",
    ] {
        let out = run(&[command, "--help"]);
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("usage: fetchira"));
    }
    // Mistyped control commands must fail before starting a server, changing config,
    // contacting an endpoint, or opening the browser login flow.
    for (args, expected) in [
        (vec!["server", "bogus"], "usage: fetchira server"),
        (vec!["server", "password"], "usage: fetchira server"),
        (vec!["server", "key"], "usage: fetchira server"),
        (
            vec!["server", "password", "hash", "extra"],
            "usage: fetchira server password hash",
        ),
        (
            vec!["server", "healthcheck", "extra"],
            "usage: fetchira server healthcheck",
        ),
        (vec!["serve-http", "extra"], "usage: fetchira serve-http"),
        (
            vec!["remote", "set", "https://example.org/mcp", "--key"],
            "missing value for --key",
        ),
        (
            vec!["remote", "login", "challenge", "--file"],
            "missing value for --file",
        ),
        (
            vec!["remote", "login", "challenge", "--browser", "--file", "x"],
            "missing value for --browser",
        ),
        (
            vec!["remote", "check", "extra"],
            "usage: fetchira remote check",
        ),
        (
            vec!["remote", "disconnect", "extra"],
            "usage: fetchira remote disconnect",
        ),
        (vec!["add", "serper", "--key"], "missing value for --key"),
        (
            vec!["add", "serper", "--label", "--key", "x"],
            "missing value for --label",
        ),
        (
            vec!["session", "account", "--file"],
            "missing value for --file",
        ),
        (vec!["setup", "extra"], "usage: fetchira setup"),
        (vec!["providers", "extra"], "usage: fetchira providers"),
        (vec!["list", "extra"], "usage: fetchira list"),
        (vec!["accounts", "extra"], "usage: fetchira accounts"),
        (vec!["install", "extra"], "usage: fetchira install"),
        (vec!["remove", "missing", "extra"], "usage: fetchira remove"),
        (vec!["login", "missing", "extra"], "usage: fetchira login"),
        (
            vec!["proxy", "missing", "direct", "extra"],
            "usage: fetchira proxy",
        ),
        (vec!["ui", "extra"], "usage: fetchira ui"),
        (vec!["serve", "extra"], "usage: fetchira serve"),
        (vec!["version", "extra"], "usage: fetchira --version"),
        (vec!["help", "extra"], "usage: fetchira help"),
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(expected),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!home.join("fetchira.toml").exists());
        assert!(!home.join("usage.db").exists());
    }
    std::fs::write(
        home.join("fetchira.toml"),
        "[remote]\nendpoint = 'http://127.0.0.1:9/mcp'\n",
    )
    .unwrap();
    for args in [
        vec!["search", "q"],
        vec!["read", "https://example.org"],
        vec!["dr", "q"],
        vec!["browser", "https://example.org"],
        vec!["create_image", "q"],
        vec!["usage"],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("remote API key is not configured"));
        assert!(!home.join("usage.db").exists());
    }
    std::fs::write(home.join("fetchira.toml"), "").unwrap();
    for (args, expected) in [
        (vec!["usage"], fetchira::router::compact_usage(&[])),
        (
            vec!["usage", "serper"],
            fetchira::router::provider_sheet(fetchira::providers::ProviderKind::Serper, &[]),
        ),
    ] {
        let out = run(&args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stderr.is_empty());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("{expected}\n")
        );
    }
    for alias in ["list", "accounts"] {
        let out = run(&[alias]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("No accounts yet"));
    }
    let out = run(&["search", "hello"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(!out.stderr.is_empty());
    std::fs::remove_dir_all(home).unwrap();
}
