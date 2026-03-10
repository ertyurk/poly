use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use clap::Parser;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use polymarket_bot::dashboard::load_dashboard_payload;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

const DASHBOARD_HTML: &str = include_str!("../../assets/dashboard.html");

type Body = Full<Bytes>;

#[derive(Debug, Parser)]
#[command(name = "dashboard", about = "Serve a local profitability dashboard")]
struct Args {
    /// Path to the SQLite database.
    #[arg(long, default_value = "data/bot.db")]
    db: String,

    /// Host interface to bind.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to serve on.
    #[arg(long, default_value_t = 3030)]
    port: u16,
}

#[derive(Debug)]
struct AppState {
    db_path: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("polymarket_bot=info".parse()?),
        )
        .init();

    let preview = load_dashboard_payload(&args.db)?;
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let listener = TcpListener::bind(addr).await?;
    let state = Arc::new(AppState {
        db_path: args.db.clone(),
    });

    tracing::info!(
        url = %format!("http://{}:{}", args.host, args.port),
        db = %args.db,
        trades = preview.trades.len(),
        skips = preview.skips.len(),
        rejections = preview.fill_rejections.len(),
        "profitability dashboard ready"
    );
    tracing::info!("press Ctrl+C to stop the dashboard server");

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, peer_addr) = accept?;
                let io = TokioIo::new(stream);
                let app_state = Arc::clone(&state);

                tokio::spawn(async move {
                    let service = service_fn(move |req| handle_request(req, Arc::clone(&app_state)));
                    if let Err(error) = http1::Builder::new().serve_connection(io, service).await {
                        tracing::warn!(peer = %peer_addr, error = %error, "dashboard connection error");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("dashboard server shutting down");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_request(
    req: Request<Incoming>,
    state: Arc<AppState>,
) -> Result<Response<Body>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/") | (&Method::GET, "/index.html") => {
            html_response(StatusCode::OK, DASHBOARD_HTML)
        }
        (&Method::GET, "/api/bootstrap") => match load_dashboard_payload(&state.db_path) {
            Ok(payload) => match serde_json::to_vec(&payload) {
                Ok(body) => response_with(
                    StatusCode::OK,
                    "application/json; charset=utf-8",
                    Bytes::from(body),
                ),
                Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
            },
            Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
        },
        (&Method::GET, "/api/health") => response_with(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            Bytes::from_static(b"ok"),
        ),
        (&Method::GET, "/favicon.ico") => response_with(
            StatusCode::NO_CONTENT,
            "text/plain; charset=utf-8",
            Bytes::new(),
        ),
        _ => error_response(StatusCode::NOT_FOUND, "route not found"),
    };

    Ok(response)
}

fn html_response(status: StatusCode, body: &str) -> Response<Body> {
    response_with(
        status,
        "text/html; charset=utf-8",
        Bytes::copy_from_slice(body.as_bytes()),
    )
}

fn error_response(status: StatusCode, message: &str) -> Response<Body> {
    match serde_json::to_vec(&serde_json::json!({ "error": message })) {
        Ok(body) => response_with(status, "application/json; charset=utf-8", Bytes::from(body)),
        Err(_) => response_with(
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            Bytes::from_static(b"internal server error"),
        ),
    }
}

fn response_with(status: StatusCode, content_type: &'static str, body: Bytes) -> Response<Body> {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
