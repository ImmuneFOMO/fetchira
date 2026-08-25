#[test]
fn hosted_compose_defaults_are_nginx_safe() {
    let yml = include_str!("../docker-compose.hosted.yml");
    assert!(
        yml.contains("profiles: [\"caddy\"]"),
        "bundled Caddy must be opt-in so an existing nginx keeps 80/443"
    );
    assert!(
        yml.contains("${FETCHIRA_PORT:-127.0.0.1:7879}:7879"),
        "default publish is loopback, not public 7879"
    );
    assert!(
        !yml.contains("${FETCHIRA_HOST:?"),
        "FETCHIRA_HOST must not be required to start Fetchira without Caddy"
    );
}

#[test]
fn hosted_docs_describe_both_install_tracks() {
    let docs = include_str!("../docs/hosted.md");
    assert!(docs.contains("Track A — fresh VPS, bundled Caddy"));
    assert!(docs.contains("Track B — existing nginx"));
    assert!(docs.contains("proxy_buffering off"));
    assert!(docs.contains("--profile caddy"));
    assert!(docs.contains("Google Chrome"));
    assert!(!docs.contains("${FETCHIRA_HOST:?"));
    assert!(
        docs.contains("first-visit setup") && docs.contains(": > secrets/admin-password"),
        "docs must describe empty admin-password and browser first-visit setup"
    );
}
