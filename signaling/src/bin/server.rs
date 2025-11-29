use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Json;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use rtc::Client;
use core::panic;
use std::collections::HashMap;
use std::io::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use str0m::change::SdpOffer;
use tokio::signal;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone, Copy)]
struct Ports {
    https: u16,
}

#[derive(Clone)]
pub struct AppState {
    pub client_channel: Sender<Client>,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let ports = Ports { https: 3000 };

    let (tx, rx): (Sender<Client>, mpsc::Receiver<Client>) = mpsc::channel(10);
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
        .with_state(state)
        .route("/whep", post(whep));

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
    let https_server = axum_server::bind_rustls(addr, config)
        .handle(handle)
        .serve(app.into_make_service());

    let mut set = JoinSet::new();
    set.spawn(process_clients(rx, token.clone()));
    set.spawn(https_server);

    set.join_all().await;

    Ok(())
}

/// WHIP endpoint
async fn whip(State(state): State<AppState>, Json(payload): Json<SdpOffer>) -> Response<String> {
    let mut client = Client::new().await.expect("Failed to create client");
    let answer = client.accept_whip_request(payload).await.unwrap();

    state.client_channel.send(client).await.unwrap();

    Response::builder()
        .status(201)
        .header("Location", "/")
        .body(answer)
        .unwrap()
}

/// WHEP endpoint
async fn whep(Json(payload): Json<SdpOffer>) -> Json<SdpAnswer> {
    tracing::info!("WHEP endpoint called: {:?}", payload);
    todo!()
}

async fn process_clients(
    mut client_channel: Receiver<Client>,
    token: CancellationToken,
) -> Result<(), std::io::Error> {
    let mut clients = HashMap::new();
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            _ = token.cancelled() => {
                tracing::debug!("Received cancellation request, shutting down SFU...");
                return Ok(());
            }

            // * Try and receive a new client
            client = client_channel.recv() => {
                match client {
                    Some(client) => {
                        tracing::trace!("New client: {:?}", client.id);
                        clients.insert(client.id, client);
                    }
                    None => {
                        tracing::debug!("Client channel closed, shutting down client processor...");
                        return Ok(());
                    }
                }
            }
            _ = interval.tick() => {
                // * Prune dead clients
                clients.retain(|id, client| {
                    if !client.rtc.is_alive() {
                        tracing::trace!("Pruning client: {id}");
                        false
                    } else {
                        true
                    }
                });
                // * Process each client
                for (_id, client) in clients.iter_mut() {
                    if let Err(e) = client.run(token.clone()).await {
                        // TODO: handle ICE disconnections more gracefully
                        tracing::debug!("Client ran into error: {:?}", e);
                    }
                }
            }
        }
    }
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

    tracing::info!("Received termination signal shutting down");
    token.cancel();
}
