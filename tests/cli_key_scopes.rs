use std::process::Command;

#[tokio::test]
async fn cli_account_management_requires_explicit_opt_in() {
    let home = std::env::temp_dir().join(format!(
        "fetchira_cli_scopes_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&home).unwrap();
    for (id, args, expected) in [
        ("friend", vec![], vec!["mcp", "usage:read"]),
        (
            "owner",
            vec!["Owner laptop", "--accounts-manage"],
            vec!["mcp", "usage:read", "accounts:manage"],
        ),
        (
            "owner2",
            vec!["--accounts-manage"],
            vec!["mcp", "usage:read", "accounts:manage"],
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_fetchira"))
            .args(["server", "key", "create", id])
            .args(args)
            .env("FETCHIRA_HOME", &home)
            .current_dir(&home)
            .output()
            .unwrap();
        assert!(output.status.success(), "key creation failed for {id}");
        let plaintext = String::from_utf8(output.stdout).unwrap();
        let store = fetchira::usage::Store::open(home.join("usage.db").to_str().unwrap())
            .await
            .unwrap();
        let key = store.api_key(id).await.unwrap().unwrap();
        assert_eq!(key.scopes, expected);
        assert!(fetchira::auth::verify_key(
            plaintext.trim(),
            &key.secret_hash
        ));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_fetchira"))
        .args(["server", "key", "create", "invalid", "--unknown"])
        .env("FETCHIRA_HOME", &home)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let store = fetchira::usage::Store::open(home.join("usage.db").to_str().unwrap())
        .await
        .unwrap();
    assert!(store.api_key("invalid").await.unwrap().is_none());
    drop(store);
    std::fs::remove_dir_all(home).unwrap();
}
