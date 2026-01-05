use anyhow::Result;
use async_channel::{self as channel, Receiver, Sender};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use axum_server::tls_rustls::RustlsConfig;
use rtc::Client;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use str0m::change::SdpOffer;
use tokio::signal;
use tokio::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use crate::session::Session;
use crate::sfu::process_sessions;

mod session;
mod sfu;

#[derive(Clone, Copy)]
struct Ports {
    https: u16,
}

#[derive(Clone)]
pub struct AppState {
    session_tx: Sender<Session>,
    session_registry: Arc<RwLock<HashMap<Uuid, UnboundedSender<Client>>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Channel for sending sessions to SFU
    let (session_tx, session_rx): (Sender<Session>, Receiver<Session>) = channel::bounded(10);
    let state = AppState {
        session_tx: session_tx.clone(),
        session_registry: Arc::new(RwLock::new(HashMap::new())),
    };

    // configure certificate and private key used by https
    let certificate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("self_signed_certs");
    let config = RustlsConfig::from_pem_file(
        PathBuf::from(&certificate_dir).join("cert.pem"),
        PathBuf::from(&certificate_dir).join("key.pem"),
    )
    .await?;

    let app = Router::new()
        .route("/whip", post(whip))
        .with_state(state.clone())
        .route("/whep", post(whep))
        .with_state(state.clone())
        .route("/sessions", get(session_list))
        .with_state(state);

    let ports = Ports { https: 3000 };
    let addr = SocketAddr::from(([127, 0, 0, 1], ports.https));
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
    // TODO: spawn an actor that will handle session tracking information. Will need to clone handles to app state and SFU
    set.spawn(process_sessions(session_rx, token.clone()));
    set.spawn(https_server);

    // TODO: refactor to a looped join_next() so we can handle errors
    set.join_all().await;

    Ok(())
}

/// WHIP endpoint
async fn whip(State(state): State<AppState>, Json(payload): Json<SdpOffer>) -> Response<String> {
    let mut client = Client::new().await.expect("Failed to create client");
    let answer = client.accept_request(payload).await.unwrap();

    // TODO: does it make sense for the HTTP server to handle session tracking?
    // or should we offload that to an actor that keeps track of them?
    let (session, subscriber_channel) = Session::new(client);
    let mut session_registry = state.session_registry.write().await;
    session_registry.insert(session.id.clone(), subscriber_channel);

    state.session_tx.send(session).await.unwrap();

    Response::builder()
        .status(201)
        .header("Location", "/") // TODO: should point to the newly created resource, but where is that? I think this would be the client/session ID so that a DELETE request could end the session
        .body(answer)
        .unwrap()
}

async fn session_list(State(state): State<AppState>) -> Json<Vec<Uuid>> {
    let session_registry = state.session_registry.read().await;
    let session_ids: Vec<Uuid> = session_registry.keys().copied().collect();
    Json(session_ids)
}

/// WHEP endpoint
async fn whep(State(state): State<AppState>, Json(payload): Json<SdpOffer>) -> Response<String> {
    // TODO: before even creating a new client, check that the session is valid
    let _session_registry = state.session_registry.read().await;

    let mut client = Client::new().await.expect("Failed to create client");
    let answer = client.accept_request(payload).await.unwrap();

    // TODO: add the client to the session

    Response::builder()
        .status(201)
        .header("Location", "/") // TODO: should point to the newly created resource, but where is that?
        .body(answer)
        .unwrap()
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
