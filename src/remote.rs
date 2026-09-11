use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use rmcp::transport::{
    streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, ClientNotification, ClientRequest, Content,
        RawContent, ResourceContents, ServerInfo, ServerResult,
    },
    service::{NotificationContext, RequestContext},
    transport::stdio,
    ErrorData, Peer, RoleClient, RoleServer, Service, ServiceError, ServiceExt,
};
use serde::{Deserialize, Serialize};

use crate::config::{self, Config};

#[derive(Debug, Deserialize)]
struct VersionResponse {
    server_version: String,
    protocol_version: String,
    schema_version: i64,
    #[serde(default)]
    min_client_version: Option<String>,
    #[serde(default)]
    max_client_version: Option<String>,
    #[serde(default)]
    min_client_schema_version: Option<i64>,
    #[serde(default)]
    max_client_schema_version: Option<i64>,
}

pub async fn check(home: &Path) -> anyhow::Result<()> {
    let cfg = config::load(home.join("fetchira.toml").to_str().unwrap_or_default())
        .context("remote is not configured")?;
    println!("{}", verify(&cfg).await?);
    Ok(())
}

/// Verify the control-plane contract before opening the MCP transport. The stdio bridge calls this
/// on every start, so an incompatible server or revoked key fails before any MCP bytes are emitted.
pub async fn verify(cfg: &Config) -> anyhow::Result<String> {
    let endpoint = cfg
        .remote
        .endpoint
        .as_deref()
        .context("remote endpoint is not configured")?;
    let base = server_base(endpoint)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let key = cfg
        .remote
        .api_key
        .as_deref()
        .context("remote API key is not configured")
        .and_then(|key| config::resolve_secret(key).map_err(Into::into))?;
    let status = client
        .get(format!("{base}/version"))
        .send()
        .await
        .context("cannot reach remote version endpoint")?;
    if !status.status().is_success() {
        bail!("remote version check failed: HTTP {}", status.status());
    }
    let remote: VersionResponse = status.json().await?;
    validate_compatibility(&remote)?;
    let auth = client
        .get(format!("{base}/auth/check"))
        .bearer_auth(&key)
        .send()
        .await
        .context("cannot reach remote authentication endpoint")?;
    if !auth.status().is_success() {
        let status = auth.status();
        let reason = auth.text().await.unwrap_or_default();
        let reason = reason.trim().chars().take(256).collect::<String>();
        bail!(
            "remote API key check failed: HTTP {status}{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(" ({reason})")
            }
        );
    }
    let checked = client
        .get(format!("{base}/remote/check"))
        .bearer_auth(&key)
        .send()
        .await
        .context("cannot reach remote connection check")?;
    if !checked.status().is_success() {
        bail!("remote connection check failed: HTTP {}", checked.status());
    }
    Ok(format!(
        "remote {} (protocol {}, schema {}) — compatible",
        remote.server_version, remote.protocol_version, remote.schema_version
    ))
}

pub fn set(home: &Path, endpoint: String, api_key: Option<String>) -> anyhow::Result<()> {
    let path = home.join("fetchira.toml");
    let mut cfg = if path.exists() {
        config::load(path.to_str().unwrap_or_default())?
    } else {
        Config::default()
    };
    apply_connection(&mut cfg, &endpoint, api_key)?;
    config::save(&cfg, &path)?;
    println!(
        "saved remote endpoint{}",
        if cfg.remote.api_key.is_some() {
            " and API key"
        } else {
            ""
        }
    );
    Ok(())
}

fn apply_connection(
    cfg: &mut Config,
    endpoint: &str,
    api_key: Option<String>,
) -> anyhow::Result<()> {
    let endpoint = normalize_endpoint(endpoint)?;
    let endpoint_changed = cfg.remote.endpoint.as_deref() != Some(endpoint.as_str());
    cfg.remote.endpoint = Some(endpoint);
    if let Some(api_key) = api_key {
        validate_api_key(&api_key)?;
        cfg.remote.api_key = Some(api_key);
    } else if endpoint_changed {
        cfg.remote.api_key = None;
    }
    Ok(())
}

pub fn disconnect(home: &Path) -> anyhow::Result<()> {
    let path = home.join("fetchira.toml");
    let mut cfg = config::load(path.to_str().unwrap_or_default())?;
    cfg.remote = Default::default();
    config::save(&cfg, &path)?;
    println!("remote disconnected");
    Ok(())
}

pub fn transport(cfg: &Config) -> anyhow::Result<StreamableHttpClientTransport<reqwest::Client>> {
    let endpoint = cfg
        .remote
        .endpoint
        .as_deref()
        .context("remote endpoint is not configured")?;
    let endpoint = normalize_endpoint(endpoint)?;
    let key = cfg
        .remote
        .api_key
        .as_deref()
        .context("remote API key is not configured")?;
    let key = config::resolve_secret(key)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(Arc::<str>::from(endpoint))
            .auth_header(key)
            .reinit_on_expired_session(true),
    );
    Ok(transport)
}

/// Call one hosted MCP tool and close the short-lived client connection.
pub async fn call_tool(
    cfg: &Config,
    name: &str,
    arguments: rmcp::model::JsonObject,
) -> anyhow::Result<CallToolResult> {
    verify(cfg).await?;
    let client = ().serve(transport(cfg)?).await.context("remote MCP handshake failed")?;
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(arguments))
        .await
        .context("remote MCP tool call failed");
    let _ = client.cancel().await;
    result
}

/// Validate a prospective remote configuration before persisting it.
pub async fn validate_config(cfg: &Config) -> anyhow::Result<String> {
    verify(cfg).await
}

/// Run the local stdio side as an MCP server and forward its requests to the hosted MCP peer.
/// The hosted connection is initialized independently; the local initialize request is answered
/// from the hosted server's negotiated info instead of being sent a second time upstream.
pub async fn serve_stdio(cfg: &Config) -> anyhow::Result<()> {
    let remote = ().serve(transport(cfg)?).await?;
    let info = remote
        .peer()
        .peer_info()
        .map(|info| (*info).clone())
        .context("remote MCP handshake returned no server info")?;
    let bridge = StdioBridge {
        peer: remote.peer().clone(),
        info,
    };
    let local = bridge.serve(stdio()).await?;
    let local_result = local.waiting().await;
    let _ = remote.cancel().await;
    local_result?;
    Ok(())
}

struct StdioBridge {
    peer: Peer<RoleClient>,
    info: ServerInfo,
}

impl Service<RoleServer> for StdioBridge {
    async fn handle_request(
        &self,
        request: ClientRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, ErrorData> {
        if matches!(request, ClientRequest::InitializeRequest(_)) {
            return Ok(ServerResult::InitializeResult(self.info.clone()));
        }
        let image_path = match &request {
            ClientRequest::CallToolRequest(call) if call.params.name.as_ref() == "create_image" => {
                call.params
                    .arguments
                    .as_ref()
                    .and_then(|args| args.get("path"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            }
            _ => None,
        };
        let result = self
            .peer
            .send_request(request)
            .await
            .map_err(remote_error)?;
        Ok(match (image_path, result) {
            (Some(path), ServerResult::CallToolResult(result)) => {
                ServerResult::CallToolResult(materialize_remote_artifacts(result, Some(path)))
            }
            (None, ServerResult::CallToolResult(result)) => {
                ServerResult::CallToolResult(materialize_remote_artifacts(result, None))
            }
            (_, result) => result,
        })
    }

    async fn handle_notification(
        &self,
        notification: ClientNotification,
        _context: NotificationContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        // The hosted peer already completed its own initialized handshake. Forwarding a second
        // initialized notification is unnecessary; other notifications remain useful upstream.
        if matches!(notification, ClientNotification::InitializedNotification(_)) {
            return Ok(());
        }
        self.peer
            .send_notification(notification)
            .await
            .map_err(remote_error)
    }

    fn get_info(&self) -> ServerInfo {
        self.info.clone()
    }
}

/// Hosted MCP returns artifact bytes over the wire. Materialize them on the user's machine so the
/// local stdio bridge has the same file contract as a non-hosted Fetchira binary.
fn materialize_remote_artifacts(
    mut result: rmcp::model::CallToolResult,
    requested_path: Option<String>,
) -> rmcp::model::CallToolResult {
    let artifact = result
        .content
        .iter()
        .find_map(|content| match &content.raw {
            RawContent::Image(image) => Some((image.data.clone(), image.mime_type.clone())),
            RawContent::Resource(resource) => match &resource.resource {
                ResourceContents::BlobResourceContents {
                    blob,
                    mime_type: Some(mime),
                    ..
                } => Some((blob.clone(), mime.clone())),
                _ => None,
            },
            _ => None,
        });
    let Some((data, mime)) = artifact else {
        return result;
    };
    let explicit_path = requested_path.is_some();
    let artifact = crate::providers::OutImage { mime, b64: data };
    let note = match crate::invoke::save_image(
        &artifact,
        requested_path.as_deref().map(std::path::Path::new),
    ) {
        Ok(note) => Content::text(note),
        Err(error) => {
            return rmcp::model::CallToolResult::error(vec![Content::text(format!(
                "hosted artifact save failed: {error:#}"
            ))]);
        }
    };
    if explicit_path {
        // Keep the hosted session token alongside the local save note. Without it a caller that
        // selected `path` cannot continue the ChatGPT image conversation for an edit.
        result
            .content
            .retain(|content| matches!(&content.raw, RawContent::Text(_)));
        result.content.insert(0, note);
    } else {
        result.content.push(note);
    }
    result
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use base64::Engine;
    use rmcp::model::RawContent;

    #[test]
    fn hosted_image_is_saved_by_local_bridge() {
        let mut result = rmcp::model::CallToolResult::success(vec![
            Content::image(
                base64::engine::general_purpose::STANDARD.encode(b"png"),
                "image/png",
            ),
            Content::text("session: chatgpt_web:test"),
        ]);
        let path =
            std::env::temp_dir().join(format!("fetchira-remote-image-{}.png", std::process::id()));
        result = materialize_remote_artifacts(result, Some(path.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read(&path).unwrap(), b"png");
        assert!(matches!(result.content[0].raw, RawContent::Text(_)));
        assert_eq!(result.content.len(), 2);
        assert!(matches!(result.content[1].raw, RawContent::Text(_)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hosted_pdf_blob_is_saved_by_local_bridge() {
        let pdf = base64::engine::general_purpose::STANDARD.encode(b"%PDF-test");
        let mut result = rmcp::model::CallToolResult::success(vec![Content::resource(
            ResourceContents::blob(pdf, "fetchira:///artifact.pdf")
                .with_mime_type("application/pdf"),
        )]);
        let path =
            std::env::temp_dir().join(format!("fetchira-remote-pdf-{}.pdf", std::process::id()));
        result = materialize_remote_artifacts(result, Some(path.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-test");
        assert_eq!(result.content.len(), 1);
        assert!(matches!(result.content[0].raw, RawContent::Text(_)));
        let _ = std::fs::remove_file(path);
    }
}

fn remote_error(error: ServiceError) -> ErrorData {
    match error {
        ServiceError::McpError(error) => error,
        error => ErrorData::internal_error(format!("hosted MCP connection failed: {error}"), None),
    }
}

fn validate_compatibility(remote: &VersionResponse) -> anyhow::Result<()> {
    if remote.protocol_version != crate::hosted::PROTOCOL_VERSION {
        bail!(
            "incompatible remote protocol: server {}, client {}; update the older side",
            remote.protocol_version,
            crate::hosted::PROTOCOL_VERSION
        );
    }
    let has_schema_range =
        remote.min_client_schema_version.is_some() || remote.max_client_schema_version.is_some();
    let incompatible_schema = if has_schema_range {
        remote
            .min_client_schema_version
            .is_some_and(|min| crate::usage::SCHEMA < min)
            || remote
                .max_client_schema_version
                .is_some_and(|max| crate::usage::SCHEMA > max)
    } else {
        remote.schema_version > crate::usage::SCHEMA
    };
    if incompatible_schema {
        bail!(
            "incompatible remote schema: server {}, client {}; update the older side",
            remote.schema_version,
            crate::usage::SCHEMA
        );
    }
    let local = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    if remote
        .min_client_version
        .as_deref()
        .map(semver::Version::parse)
        .transpose()?
        .is_some_and(|min| local < min)
        || remote
            .max_client_version
            .as_deref()
            .map(semver::Version::parse)
            .transpose()?
            .is_some_and(|max| local > max)
    {
        bail!(
            "incompatible fetchira client {}; server accepts {}..{}",
            local,
            remote.min_client_version.as_deref().unwrap_or("any"),
            remote.max_client_version.as_deref().unwrap_or("any")
        );
    }
    Ok(())
}

fn server_base(endpoint: &str) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(&normalize_endpoint(endpoint)?)?;
    let path = url.path().trim_end_matches('/');
    let prefix = path
        .strip_suffix("/mcp")
        .ok_or_else(|| anyhow::anyhow!("remote endpoint path must end in /mcp"))?
        .to_string();
    url.set_path(&prefix);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn normalize_endpoint(endpoint: &str) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(endpoint.trim()).context("invalid remote endpoint URL")?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("remote endpoint must use HTTPS (or loopback HTTP)");
    }
    if url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("remote endpoint cannot contain credentials, query, or fragment");
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(if path.ends_with("/mcp") {
        &path
    } else if path.is_empty() {
        "/mcp"
    } else {
        return Err(anyhow::anyhow!("remote endpoint path must be /mcp"));
    });
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn validate_api_key(key: &str) -> anyhow::Result<()> {
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        bail!("invalid Fetchira API key (expected fk_live_*)");
    }
    // Keep supported secret references (`env:NAME` and `enc:...`) in the config while
    // validating the resolved credential that the transport will actually send.
    let resolved = config::resolve_secret(key).context("invalid Fetchira API key")?;
    if !resolved.starts_with("fk_live_") || resolved.chars().any(char::is_whitespace) {
        bail!("invalid Fetchira API key (expected fk_live_*)");
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct LoginChallenge {
    pub provider: crate::providers::ProviderKind,
    pub label: String,
    pub expires_at: String,
}

#[derive(Serialize)]
struct LoginUpload<'a> {
    session: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limits: Option<serde_json::Value>,
}

pub async fn login(
    home: &Path,
    challenge: &str,
    file: Option<&str>,
    browser: Option<&str>,
) -> anyhow::Result<()> {
    if challenge.is_empty()
        || challenge.len() > 256
        || !challenge
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        bail!("invalid login challenge");
    }
    let cfg = config::load(home.join("fetchira.toml").to_str().unwrap_or_default())?;
    let endpoint = cfg
        .remote
        .endpoint
        .as_deref()
        .context("remote endpoint is not configured")?;
    let key = cfg
        .remote
        .api_key
        .as_deref()
        .context("remote API key is not configured")
        .and_then(|key| config::resolve_secret(key).map_err(Into::into))?;
    let url = format!("{}/login-challenges/{challenge}", server_base(endpoint)?);
    let client = reqwest::Client::new();
    let response = client.get(&url).bearer_auth(&key).send().await?;
    if !response.status().is_success() {
        bail!("login challenge lookup failed: HTTP {}", response.status());
    }
    let target: LoginChallenge = response.json().await?;
    let browser = browser.map(|value| match value {
        "chromium" => "chrome",
        "ff" => "firefox",
        other => other,
    });
    let session = match file {
        Some(path) => std::fs::read_to_string(path).with_context(|| format!("read {path}"))?,
        None => {
            let captured = crate::web::login(
                home,
                target.provider,
                &target.label,
                browser.map(str::to_string),
                None,
            )
            .await?;
            serde_json::to_string(&captured)?
        }
    };
    let parsed = crate::web::parse_session(&session);
    if parsed.cookies.is_empty() {
        bail!("session contains no cookies");
    }
    let (identity, plan, limits) =
        if let Ok(http) = crate::web::build_client(&parsed.cookies, &parsed.headers, None) {
            let p = crate::providers::Provider::new(target.provider);
            let identity = p.account_identity(&http).await;
            let ll = p
                .live_limits(&http, &target.label, &parsed.cookies, false)
                .await;
            let plan = ll
                .as_ref()
                .and_then(|l| l.tier.clone())
                .filter(|t| t != "free");
            let limits = ll
                .as_ref()
                .filter(|l| l.has_quotas())
                .and_then(|l| serde_json::to_value(l).ok());
            (identity, plan, limits)
        } else {
            (None, None, None)
        };
    let response = client
        .post(url)
        .bearer_auth(key)
        .json(&LoginUpload {
            session: &session,
            identity: identity.as_deref(),
            plan: plan.as_deref(),
            limits,
        })
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("login session upload failed: HTTP {}", response.status());
    }
    println!("remote session uploaded for '{}'", target.label);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::get, Json, Router as AxumRouter};
    use rmcp::model::ServerCapabilities;
    use rmcp::service::{NotificationContext, RequestContext};
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };
    use rmcp::{RoleServer, Service};
    use serde_json::json;

    #[test]
    fn bridge_preserves_hosted_protocol_errors() {
        let error = ErrorData::invalid_params("local stdio required", None);
        assert_eq!(remote_error(ServiceError::McpError(error.clone())), error);
    }

    #[test]
    fn endpoint_validation_is_strict() {
        assert_eq!(
            normalize_endpoint("https://example.test").unwrap(),
            "https://example.test/mcp"
        );
        assert_eq!(
            normalize_endpoint("http://127.0.0.1:7879/mcp").unwrap(),
            "http://127.0.0.1:7879/mcp"
        );
        assert!(normalize_endpoint("http://example.test/mcp").is_err());
        assert!(normalize_endpoint("http://[2001:db8::1]/mcp").is_err());
        assert!(normalize_endpoint("https://user:secret@example.test/mcp").is_err());
        assert!(normalize_endpoint("https://example.test/other").is_err());
        assert_eq!(
            server_base("http://[::1]:7879/mcp").unwrap(),
            "http://[::1]:7879"
        );
    }

    #[test]
    fn compatibility_bounds_are_enforced() {
        let mut version = VersionResponse {
            server_version: "1.0.0".into(),
            protocol_version: crate::hosted::PROTOCOL_VERSION.into(),
            schema_version: crate::usage::SCHEMA,
            min_client_version: None,
            max_client_version: None,
            min_client_schema_version: None,
            max_client_schema_version: None,
        };
        assert!(validate_compatibility(&version).is_ok());
        version.min_client_schema_version = Some(crate::usage::SCHEMA + 1);
        assert!(validate_compatibility(&version).is_err());
        version.min_client_schema_version = Some(1);
        version.max_client_schema_version = Some(crate::usage::SCHEMA);
        version.schema_version = crate::usage::SCHEMA + 1;
        assert!(validate_compatibility(&version).is_ok());
        version.min_client_schema_version = None;
        version.max_client_schema_version = None;
        assert!(validate_compatibility(&version).is_err());
    }

    #[test]
    fn changing_remote_endpoint_drops_implicit_old_key() {
        let mut cfg = Config::default();
        apply_connection(
            &mut cfg,
            "https://old.example.test/mcp",
            Some("fk_live_old".into()),
        )
        .unwrap();
        apply_connection(&mut cfg, "https://old.example.test", None).unwrap();
        assert_eq!(cfg.remote.api_key.as_deref(), Some("fk_live_old"));
        apply_connection(&mut cfg, "https://new.example.test", None).unwrap();
        assert!(cfg.remote.api_key.is_none());
    }

    #[derive(Clone)]
    struct EchoMcp {
        seen: Arc<tokio::sync::Mutex<Option<rmcp::model::JsonObject>>>,
    }

    impl Service<RoleServer> for EchoMcp {
        async fn handle_request(
            &self,
            request: ClientRequest,
            _context: RequestContext<RoleServer>,
        ) -> Result<ServerResult, ErrorData> {
            match request {
                ClientRequest::InitializeRequest(_) => Ok(ServerResult::InitializeResult(
                    ServerInfo::new(ServerCapabilities::builder().enable_tools().build()),
                )),
                ClientRequest::CallToolRequest(request) => {
                    *self.seen.lock().await = request.params.arguments;
                    Ok(ServerResult::CallToolResult(CallToolResult::success(vec![
                        Content::text("hosted echo"),
                    ])))
                }
                _ => Err(ErrorData::internal_error("unexpected test request", None)),
            }
        }

        async fn handle_notification(
            &self,
            _notification: ClientNotification,
            _context: NotificationContext<RoleServer>,
        ) -> Result<(), ErrorData> {
            Ok(())
        }

        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }
    }

    #[tokio::test]
    async fn hosted_call_preflights_and_uses_streamable_http() {
        let seen = Arc::new(tokio::sync::Mutex::new(None));
        let mcp = StreamableHttpService::new(
            {
                let seen = seen.clone();
                move || Ok(EchoMcp { seen: seen.clone() })
            },
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let app = AxumRouter::new()
            .route(
                "/version",
                get(|| async {
                    Json(json!({
                        "server_version": "test",
                        "protocol_version": crate::hosted::PROTOCOL_VERSION,
                        "schema_version": crate::usage::SCHEMA,
                        "min_client_schema_version": 1,
                        "max_client_schema_version": crate::usage::SCHEMA
                    }))
                }),
            )
            .route("/auth/check", get(|| async { StatusCode::OK }))
            .route("/remote/check", get(|| async { StatusCode::OK }))
            .nest_service("/mcp", mcp);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut cfg = Config::default();
        cfg.remote.endpoint = Some(format!("http://{addr}/mcp"));
        cfg.remote.api_key = Some("fk_live_test".into());
        let mut args = rmcp::model::JsonObject::new();
        args.insert("query".into(), json!("hosted parity"));
        let result = call_tool(&cfg, "search", args).await.unwrap();
        assert_eq!(result.content[0].raw.as_text().unwrap().text, "hosted echo");
        assert_eq!(
            seen.lock().await.as_ref().unwrap()["query"],
            "hosted parity"
        );
        server.abort();
    }

    #[tokio::test]
    async fn hosted_call_rejects_missing_key_before_handshake() {
        let mut cfg = Config::default();
        cfg.remote.endpoint = Some("http://127.0.0.1:9/mcp".into());
        let error = call_tool(&cfg, "usage", rmcp::model::JsonObject::new())
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("remote API key is not configured"));
    }
}
