use super::Server;
use crate::error::Error;
use oauth2::AccessToken;
use rmcp::{
    RoleClient, ServerHandler, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
        streamable_http_server::StreamableHttpServerConfig,
    },
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "synthetic-mcp-bearer-token";

pub(super) struct Http {
    pub client: RunningService<RoleClient, ()>,
    url: String,
    cancellation: CancellationToken,
    task: JoinHandle<std::io::Result<()>>,
}

impl Http {
    pub async fn new(server: impl ServerHandler + Clone) -> Self {
        Self::configured(server, StreamableHttpServerConfig::default()).await
    }

    async fn configured(
        server: impl ServerHandler + Clone,
        config: StreamableHttpServerConfig,
    ) -> Self {
        crate::initialize_tls();
        let cancellation = config.cancellation_token.clone();
        let router = super::super::http::router(server, AccessToken::new(TOKEN.into()), config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        let transport = StreamableHttpClientTransport::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            StreamableHttpClientTransportConfig::with_uri(url.clone()).auth_header(TOKEN),
        );
        let client = tokio::time::timeout(Duration::from_secs(5), ().serve(transport))
            .await
            .unwrap()
            .unwrap();
        Self {
            client,
            url,
            cancellation,
            task,
        }
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> CallToolResult {
        tokio::time::timeout(
            Duration::from_secs(5),
            self.client.call_tool(
                CallToolRequestParams::new(tool.to_owned())
                    .with_arguments(arguments.as_object().unwrap().clone()),
            ),
        )
        .await
        .unwrap()
        .unwrap()
    }
}

impl Drop for Http {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

#[tokio::test]
async fn mcp_http_authenticates_every_request_and_rejects_bad_hosts_origins_and_large_bodies() {
    let http = Http::configured(
        Server::new(
            "work".into(),
            |_| async { Err(Error::CredentialMissing) },
            false,
        ),
        StreamableHttpServerConfig::default().with_max_request_body_bytes(4096),
    )
    .await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "legacy-test", "version": "1"}}
    });
    let response = client
        .post(&http.url)
        .bearer_auth(TOKEN)
        .header("accept", "application/json, text/event-stream")
        .json(&initialize)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    response.bytes().await.unwrap();

    for method in [
        reqwest::Method::POST,
        reqwest::Method::GET,
        reqwest::Method::DELETE,
    ] {
        for token in [None, Some("synthetic-mcp-bearer-tokeN"), Some("")] {
            let mut request = client
                .request(method.clone(), &http.url)
                .header("mcp-session-id", &session)
                .header("accept", "application/json, text/event-stream");
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let response = request.json(&initialize).send().await.unwrap();
            assert_eq!(response.status(), 401);
            assert_eq!(response.headers()["www-authenticate"], "Bearer");
            assert!(!response.text().await.unwrap().contains(TOKEN));
        }
    }
    for (header, value) in [
        ("host", "attacker.invalid"),
        ("origin", "https://attacker.invalid"),
        ("origin", "null"),
    ] {
        let response = client
            .post(&http.url)
            .bearer_auth(TOKEN)
            .header("accept", "application/json, text/event-stream")
            .header(header, value)
            .json(&initialize)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{header}: {value}");
    }
    for streamed in [false, true] {
        let body = if streamed {
            reqwest::Body::wrap_stream(futures_util::stream::iter([
                Ok::<_, std::io::Error>(vec![b' '; 2049]),
                Ok(vec![b' '; 2048]),
            ]))
        } else {
            reqwest::Body::from(vec![b' '; 4097])
        };
        let response = client
            .post(&http.url)
            .bearer_auth(TOKEN)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 413);
    }
    let response = client
        .post(&http.url)
        .bearer_auth(TOKEN)
        .header("mcp-session-id", &session)
        .header("mcp-protocol-version", "2025-03-26")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.text().await.unwrap().contains("slk_read"));
    let tools = http.client.list_tools(None).await.unwrap().tools;
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["slk_help", "slk_read"]
    );
    assert!(
        http.client
            .call_tool(
                CallToolRequestParams::new("slk_write").with_arguments(
                    json!({"args": ["send", "C1", "--text", "must not send", "--yes"]})
                        .as_object()
                        .unwrap()
                        .clone()
                )
            )
            .await
            .is_err()
    );
    let result = http.call("slk_read", json!({"args": ["read", "C1"]})).await;
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "credential_missing"
    );
}
