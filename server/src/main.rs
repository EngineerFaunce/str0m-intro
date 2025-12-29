use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum_server::tls_rustls::RustlsConfig;
use rtc::Client;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use str0m::change::SdpOffer;
use tokio::signal;
use tokio::sync::mpsc::{self, Sender};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::sfu::process_clients;

mod sfu;

#[derive(Clone, Copy)]
struct Ports {
    https: u16,
}

#[derive(Clone)]
pub struct AppState {
    client_channel: Sender<SessionClient>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let ports = Ports { https: 3000 };

    let (tx, rx): (Sender<SessionClient>, mpsc::Receiver<SessionClient>) = mpsc::channel(10);
    let state = AppState {
        client_channel: tx.clone(),
    };
    let token = CancellationToken::new();

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
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], ports.https));
    let handle = axum_server::Handle::new();

    // * Spawn shutdown signal listener
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
    set.spawn(process_clients(rx, token.clone()));
    set.spawn(https_server);

    // TODO: refactor to a looped join_next() so we can handle errors
    set.join_all().await;

    Ok(())
}

/// WHIP endpoint
async fn whip(State(state): State<AppState>, Json(payload): Json<SdpOffer>) -> Response<String> {
    let mut client = Client::new().await.expect("Failed to create client");
    let answer = client.accept_request(payload).await.unwrap();

    state
        .client_channel
        .send(SessionClient {
            client,
            kind: SessionKind::Whip,
        })
        .await
        .unwrap();

    Response::builder()
        .status(201)
        .header("Location", "/") // TODO: should point to the newly created resource, but where is that?
        .body(answer)
        .unwrap()
}

/// WHEP endpoint
// TODO: accept a session ID to identify the session to attach to.
async fn whep(State(state): State<AppState>, Json(payload): Json<SdpOffer>) -> Response<String> {
    // TODO: before even creating a new client, check that the session is valid

    let mut client = Client::new().await.expect("Failed to create client");
    let answer = client.accept_request(payload).await.unwrap();

    // TODO:
    // state
    //     .client_channel
    //     .send(SessionClient {
    //         client,
    //         kind: SessionKind::Whep,
    //     })
    //     .await
    //     .unwrap();

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
