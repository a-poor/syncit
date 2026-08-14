use anyhow::Result;
use axum::{Json, Router, http::StatusCode, routing::get};
use serde::Serialize;
use tower_http::trace::{self, TraceLayer};
use tracing::Level;

#[tokio::main]
async fn main() -> Result<()> {
    // initialize tracing
    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .init();

    // build our application with a route
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/api/data", get(get_user))
        .route("/api/ws", get(get_user))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
        );

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3003").await?;

    tracing::info!(port = "3003", "Starting");

    axum::serve(listener, app).await?;

    Ok(())
}

async fn root() -> &'static str {
    "Hello, World!"
}

async fn get_user() -> (StatusCode, Json<User>) {
    let user = User {
        id: 1337,
        username: "foo".into(),
    };
    (StatusCode::CREATED, Json(user))
}

#[derive(Serialize)]
struct User {
    id: u64,
    username: String,
}
