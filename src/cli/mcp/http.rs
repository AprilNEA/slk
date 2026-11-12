use super::Options;
use crate::error::{Error, Result};
use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header::WWW_AUTHENTICATE},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
};
use axum_extra::{
    TypedHeader,
    headers::{Authorization, authorization::Bearer},
    typed_header::TypedHeaderRejection,
};
use oauth2::AccessToken;
use rmcp::{
    ServerHandler,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use std::{net::SocketAddr, sync::Arc};

pub(super) async fn run<S>(server: S, address: SocketAddr, options: Options) -> Result<()>
where
    S: ServerHandler + Clone,
{
    let variable = options
        .bearer_token_env
        .expect("Clap requires --bearer-token-env with --listen");
    let token = std::env::var(variable).map_err(|_| {
        Error::Arguments("The MCP bearer-token environment variable is missing or not UTF-8.")
    })?;
    if token.is_empty()
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
        || Authorization::bearer(&token).is_err()
    {
        return Err(Error::Arguments(
            "The MCP bearer token must be nonempty HTTP header text without whitespace.",
        ));
    }
    let mut config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(options.allowed_host.iter().map(ToString::to_string));
    if let Some(bytes) = options.max_request_bytes {
        config = config.with_max_request_body_bytes(bytes.get());
    }
    let cancellation = config.cancellation_token.clone();
    let router = router(server, AccessToken::new(token), config);
    let cancelled = crate::app::cancellation()?;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| Error::io("listen for MCP HTTP requests", error))?;
    let address = listener
        .local_addr()
        .map_err(|error| Error::io("inspect the MCP listening address", error))?;
    eprintln!("MCP HTTP listening on {address}/mcp");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            cancelled.await;
            cancellation.cancel();
        })
        .await
        .map_err(|error| Error::io("serve MCP HTTP requests", error))
}

pub(super) fn router<S>(server: S, token: AccessToken, config: StreamableHttpServerConfig) -> Router
where
    S: ServerHandler + Clone,
{
    let service: StreamableHttpService<S, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(server.clone()),
        Default::default(),
        config.enforce_origin_validation(),
    );
    Router::new()
        .nest_service("/mcp", service)
        .layer(from_fn_with_state(Arc::new(token), authenticate))
}

async fn authenticate(
    State(expected): State<Arc<AccessToken>>,
    authorization: std::result::Result<TypedHeader<Authorization<Bearer>>, TypedHeaderRejection>,
    request: Request,
    next: Next,
) -> Response {
    if let Ok(TypedHeader(authorization)) = authorization
        && AccessToken::new(authorization.token().to_owned()) == *expected
    {
        return next.run(request).await;
    }
    (StatusCode::UNAUTHORIZED, [(WWW_AUTHENTICATE, "Bearer")]).into_response()
}
