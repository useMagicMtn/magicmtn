//! Templates and embedded assets used by MagicMtn dashboard hosts.
pub mod mcp;
use askama::Template;
use axum::{http::header, routing::get, Router};
use serde::Deserialize;

#[derive(Template, Deserialize)]
#[template(path = "shared/images.html")]
pub struct ImageCatalog {
    pub images: Vec<Image>,
}

#[derive(Deserialize)]
pub struct Image {
    pub name: String,
    #[serde(rename = "source")]
    pub description: String,
    pub preparations: Vec<Preparation>,
}

#[derive(Deserialize)]
pub struct Preparation {
    pub provider: String,
    pub ready: bool,
}

#[derive(Template)]
#[template(path = "shared/computers.html")]
pub struct Computers {
    pub computers: Vec<Computer>,
}

#[derive(Deserialize)]
pub struct Computer {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub template: String,
    pub created_at: Option<String>,
    pub notes: Option<String>,
}

#[derive(Template)]
#[template(path = "shared/providers.html")]
pub struct Providers {
    pub installed: Vec<ProviderRelease>,
}

pub struct ProviderRelease {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub active: bool,
}

pub fn html(page: impl Template) -> Result<axum::response::Html<String>, axum::http::StatusCode> {
    page.render()
        .map(axum::response::Html)
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Template)]
#[template(path = "shared/templates.html")]
pub struct TemplatesPage {
    pub source: String,
}

#[derive(Template)]
#[template(path = "shared/mcp.html")]
pub struct McpPage {
    pub services: &'static [mcp::McpService],
}

pub fn assets() -> Router {
    let mut router = Router::new();
    macro_rules! asset {
        ($path:literal, $mime:literal) => {
            router = router.route(
                concat!("/static/", $path),
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, $mime)],
                        include_bytes!(concat!("../static/", $path)).as_slice(),
                    )
                }),
            );
        };
    }
    asset!("templates.css", "text/css");
    asset!("mcp-catalog.js", "text/javascript; charset=utf-8");
    asset!("templates.js", "application/javascript");
    asset!("app.css", "text/css; charset=utf-8");
    asset!("dashboard.js", "text/javascript; charset=utf-8");
    asset!("favicon.svg", "image/svg+xml");
    asset!("fonts/DMMono-Medium.woff2", "font/woff2");
    asset!("fonts/DMMono-Regular.woff2", "font/woff2");
    asset!("fonts/InstrumentSerif-Italic.woff2", "font/woff2");
    asset!("fonts/InstrumentSerif-Regular.woff2", "font/woff2");
    asset!("fonts/PublicSans-Italic.woff2", "font/woff2");
    asset!("fonts/PublicSans.woff2", "font/woff2");
    asset!("fonts/ibm-plex-sans-medium.ttf", "font/ttf");
    asset!("fonts/ibm-plex-sans-regular.ttf", "font/ttf");
    asset!("furnace.css", "text/css; charset=utf-8");
    asset!("htmx.min.js", "text/javascript; charset=utf-8");
    asset!("layout.css", "text/css; charset=utf-8");
    asset!("shell.css", "text/css; charset=utf-8");
    asset!("wordmark-dark.svg", "image/svg+xml");
    asset!("wordmark.svg", "image/svg+xml");
    router
}
