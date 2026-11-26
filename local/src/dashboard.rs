//! Local browser dashboard. The browser credential is scoped to this process.
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use base64::{engine::general_purpose::STANDARD, Engine};
use magicmtn_engine::{sandbox_images, sandbox_storage::sqlite::Database};
use magicmtn_provider_framework::host::Registry;
use magicmtn_web_ui::{html, Computers, ImageCatalog, ProviderRelease, Providers};
use std::sync::Arc;

pub(crate) fn router(database: Database, registry: Registry, password: &str) -> Router {
    let authorization = format!("Basic {}", STANDARD.encode(format!("magic:{password}")));
    Router::new()
        .route(
            "/",
            get(|| async { Redirect::temporary("/compute/computers") }),
        )
        .route(
            "/mcp",
            get(|| async {
                html(magicmtn_web_ui::McpPage {
                    services: magicmtn_web_ui::mcp::SERVICES,
                })
            }),
        )
        .route("/compute/computers", get(computers))
        .route("/compute/images", get(images))
        .with_state(database.clone())
        .merge(crate::templates::router(database))
        .merge(
            Router::new()
                .route("/providers", get(providers))
                .with_state(registry),
        )
        .merge(magicmtn_web_ui::assets())
        .layer(middleware::from_fn_with_state(
            Arc::new(authorization),
            authenticate,
        ))
}

async fn images(State(database): State<Database>) -> Result<Html<String>, StatusCode> {
    let catalog = sandbox_images::list(&database)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let catalog: ImageCatalog =
        serde_json::from_value(catalog).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    html(catalog)
}

async fn computers(State(database): State<Database>) -> Result<Html<String>, StatusCode> {
    let records = database
        .records(None)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let computers = records
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    html(Computers { computers })
}

async fn providers(State(registry): State<Registry>) -> Result<Html<String>, StatusCode> {
    let releases = registry
        .list()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let installed = releases
        .into_iter()
        .map(|release| ProviderRelease {
            name: release.package.name,
            version: release.package.version,
            platform: format!("{} / {}", release.package.os, release.package.arch),
            active: release.active,
        })
        .collect();
    html(Providers { installed })
}

async fn authenticate(
    State(expected): State<Arc<String>>,
    request: Request,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .map(HeaderValue::as_bytes)
        .unwrap_or_default();
    let mut response = if super::credential_matches(supplied, expected.as_bytes()) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"MagicMtn dashboard\"",
            )],
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}
