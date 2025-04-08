use axum::response::Response;
use axum::routing::post;
use axum::Json;
use axum::{response::IntoResponse, routing::get, Router};
use axum_server::tls_rustls::RustlsConfig;
use core::panic;
use reqwest::StatusCode;
use signaling::client::Client;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread;
use std::time::Duration;
use std::{collections::HashMap, path::PathBuf};
use str0m::change::{SdpAnswer, SdpOffer};
use tokio::signal;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone, Copy)]
struct Ports {
    http: u16,
    https: u16,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let ports = Ports {
        http: 7878,
        https: 3000,
    };

    // Create a handle for our TLS server so the shutdown signal can all shutdown
    let handle = axum_server::Handle::new();
    // save the future for easy shutting down of redirect server
    let shutdown_future = shutdown_signal(handle.clone());

    // ? tx = transmission
    // ? rx = receiving
    // let (tx, rx): (SyncSender<SignalMessage>, Receiver<SignalMessage>) = mpsc::sync_channel(1);

    // Separate thread to process clients as offers are made/accepted.
    // thread::spawn(move || run(rx));

    // optional: spawn a second server to redirect http requests to this server
    // tokio::spawn(redirect_http_to_https(ports, shutdown_future));

    // configure certificate and private key used by https
    let config = RustlsConfig::from_pem_file(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("self_signed_certs")
            .join("cer.pem"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("self_signed_certs")
            .join("key.pem"),
    )
    .await
    .unwrap();

    let app = Router::new()
        .route("/health", get(health))
        .route("/whip", post(whip));

    // run https server
    let addr = SocketAddr::from(([127, 0, 0, 1], ports.https));
    tracing::debug!("listening on {addr}");
    axum_server::bind_rustls(addr, config)
        .handle(handle)
        .serve(app.into_make_service())
        .await
        .unwrap();
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

// TODO: remove this. For debugging purposes only.
async fn health() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

/// WHIP endpoint
async fn whip(Json(payload): Json<SdpOffer>) -> Response<String> {
    let mut client = Client::new().expect("Failed to create client");

    let answer = client.accept_whip_request(payload).await.unwrap();

    Response::builder()
        .status(201)
        .header("Location", "/")
        .body(answer)
        .unwrap()
}

/// WHEP endpoint
async fn whep(Json(payload): Json<SdpOffer>) -> Json<SdpAnswer> {
    todo!()
}

// fn run(rx: Receiver<SignalMessage>) {
//     let mut pending_clients: HashMap<Uuid, Client<Pending>> = HashMap::new();
//     let mut clients: HashMap<Uuid, Client<Connected>> = HashMap::new();

//     loop {
//         // Remove disconnected clients.
//         clients.retain(|_, c| c.rtc.is_alive());

//         match rx.try_recv() {
//             Ok(SignalMessage::Offer(client)) => {
//                 info!("Sent offer to client: {:?}", client.id);
//                 pending_clients.insert(client.id, client);
//             }
//             Ok(SignalMessage::Answer(answer)) => {
//                 info!("Received answer from client: {:?}", answer.id);

//                 // Accept the answer
//                 // TODO: error handling
//                 let client = pending_clients.remove(&answer.id).unwrap();
//                 let client = client
//                     .accept_answer(answer.answer)
//                     .expect("answer to be accepted");
//                 clients.insert(client.id, client);
//             }
//             Err(TryRecvError::Empty) => {}
//             Err(TryRecvError::Disconnected) => {
//                 panic!("Channel disconnected");
//             }
//         };

//         // TODO: start polling clients
//         // TODO: propagate changes to other clients
//     }
// }
