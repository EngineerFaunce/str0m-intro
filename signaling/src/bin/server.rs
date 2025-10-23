use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use axum::Json;
use axum_server::tls_rustls::RustlsConfig;
use core::panic;
use signaling::client::Client;
use std::collections::HashMap;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use str0m::change::{SdpAnswer, SdpOffer};
use tokio::signal;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tracing::debug;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[derive(Clone, Copy)]
struct Ports {
    https: u16,
}

#[derive(Clone)]
pub struct AppState {
    pub client_channel: Sender<Client>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let ports = Ports {
        https: 3000,
    };

    let (tx, rx): (Sender<Client>, mpsc::Receiver<Client>) = mpsc::channel(10);
    let state = AppState {
        client_channel: tx.clone(),
    };

    // Separate thread to process WHIP/WHEP clients
    tokio::spawn(process_clients(rx));

    // configure certificate and private key used by https
    let certificate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("self_signed_certs");
    let config = RustlsConfig::from_pem_file(
        PathBuf::from(&certificate_dir).join("cert.pem"),
        PathBuf::from(&certificate_dir).join("key.pem"),
    )
    .await
    .unwrap();

    let app = Router::new()
        .route("/whip", post(whip))
        .with_state(state)
        .route("/whep", post(whep));

    let addr = SocketAddr::from(([127, 0, 0, 1], ports.https));
    tracing::debug!("listening on {addr}");
    axum_server::bind_rustls(addr, config)
        .handle(handle)
        .serve(app.into_make_service())
        .await
        .unwrap();
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
    debug!("WHEP endpoint called: {:?}", payload);
    todo!()
}

async fn process_clients(mut client_channel: Receiver<Client>) {
    let mut clients: HashMap<Uuid, Client> = HashMap::new();
    loop {
        // * Try and receive a new client
        match client_channel.try_recv() {
            Ok(client) => {
                debug!("New client: {:?}", client.id);
                clients.insert(client.id, client);
            }
            Err(_) => {
                // TODO: handle error
            }
        }

        // * Prune dead clients
        {
            let mut targets = Vec::new();
            for (id, client) in clients.iter() {
                if !client.rtc.is_alive() {
                    targets.push(*id);
                }
            }

            for id in targets {
                debug!("Pruning client: {id}");
                clients.remove(&id);
            }
        }

        // * Process each client
        for (_id, client) in clients.iter_mut() {
            match client.run().await {
                Ok(_) => {}
                Err(e) => {
                    debug!("Client ran into error: {:?}", e);
                    continue;
                }
            }
        }
    }
}

async fn shutdown_signal(handle: axum_server::Handle) {
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

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Received termination signal shutting down");
    handle.graceful_shutdown(Some(Duration::from_secs(10))); // 10 secs is how long docker will wait
                                                             // to force shutdown
}

