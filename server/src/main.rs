use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::routing::post;
use axum_server::tls_rustls::RustlsConfig;
use rtc::Client;
use session::session_manager::SessionManager;
use session::session_manager::SessionManagerHandle;
use session::session_manager::SessionMessage;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use str0m::change::SdpOffer;
use tokio::signal;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

mod session;

#[derive(Clone, Copy)]
struct Ports {
    https: u16,
}

#[derive(Clone)]
pub struct AppState {
    session_manager: SessionManagerHandle,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    // * Custom session manager actor and a handle used for communicating with it
    let (mut session_manager, session_manager_handle) = SessionManager::new();
    let handle = axum_server::Handle::new();

    // * Spawn shutdown signal listener
    let token = CancellationToken::new();
    tokio::spawn(shutdown_signal(token.clone()));

    // * Spawn graceful shutdown task
    // ? Is this okay to have in a structured concurrency context?
    {
        let shutdown_handle = handle.clone();
        let token = token.clone();
        tokio::spawn(async move {
            token.cancelled().await;
            tracing::debug!("Received cancellation request, shutting down web server...");
            shutdown_handle.graceful_shutdown(Some(Duration::from_secs(10)));
        });
    }

    // * Web server configuration
    let state = AppState {
        session_manager: session_manager_handle.clone(),
    };
    let app = Router::new()
        .route("/whip", post(whip))
        .with_state(state.clone())
        .route("/whep/{session_id}", post(whep))
        .with_state(state.clone())
        .route("/sessions", get(session_list))
        .with_state(state);
    let ports = Ports { https: 3000 };
    let addr = SocketAddr::from(([127, 0, 0, 1], ports.https));
    let certificate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("self_signed_certs");
    let config = RustlsConfig::from_pem_file(
        PathBuf::from(&certificate_dir).join("cert.pem"),
        PathBuf::from(&certificate_dir).join("key.pem"),
    )
    .await?;

    tracing::info!("listening on {addr}");
    // ! "Stuffing" the awaited server into a separate async block in order to map the error type. Is this appropriate, or is it scuffed?
    let https_server = async move {
        axum_server::bind_rustls(addr, config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .map_err(anyhow::Error::from)
    };

    let mut set = JoinSet::new();
    set.spawn(https_server);
    // TODO: why does adding the async move (and .await) here fix the lifetime error?
    set.spawn(async move { session_manager.run(token.clone()).await });

    // TODO: refactor to a looped join_next() so we can handle errors
    set.join_all().await;

    Ok(())
}

/// WHIP endpoint
async fn whip(State(state): State<AppState>, Json(payload): Json<SdpOffer>) -> impl IntoResponse {
    let mut client = Client::new().await.expect("Failed to create client");
    let answer = client.accept_request(payload).await.unwrap();

    if let Err(_) = state
        .session_manager
        .try_send(SessionMessage::NewPublisher(client))
    {
        tracing::error!("failed to send client to session manager");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // TODO: add header pointing to newly created resource
    (StatusCode::CREATED, answer).into_response()
}

async fn session_list(State(state): State<AppState>) -> Json<Vec<Uuid>> {
    let (tx, rx) = oneshot::channel();
    if let Err(_) = state
        .session_manager
        .try_send(SessionMessage::GetActiveSessions(tx))
    {
        tracing::error!("error from requesting active sessions from session manager");
    }
    match rx.await {
        Ok(session_list) => Json(session_list),
        Err(_) => {
            tracing::error!("error fetching list of active sessions");
            Json(vec![])
        }
    }
}

/// WHEP endpoint
async fn whep(
    Path(session_id): Path<Uuid>,
    State(state): State<AppState>,
    Json(payload): Json<SdpOffer>,
) -> impl IntoResponse {
    // Before even creating a new client, check that the session is valid
    let (tx, rx) = oneshot::channel();
    if state
        .session_manager
        .send(SessionMessage::ValidateSession(session_id, tx))
        .await
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let is_valid = match rx.await {
        Ok(valid) => valid,
        Err(_) => {
            tracing::error!("error validating session");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if !is_valid {
        tracing::error!("session is invalid");
        // TODO: is this following the WHEP protocol?
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut client = Client::new().await.expect("Failed to create client");
    let answer = client.accept_request(payload).await.unwrap();

    if let Err(_) = state
        .session_manager
        .try_send(SessionMessage::NewSubscriber(session_id, client))
    {
        tracing::error!("oops")
    }

    // TODO: add "Location" header that points to newly created resource
    (StatusCode::CREATED, answer).into_response()
}

async fn shutdown_signal(token: CancellationToken) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    // TODO: provide a means for the user to shut down the server that isn't just ctrl-c
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Received termination signal. Initiating shutdown.");
    token.cancel();
}
