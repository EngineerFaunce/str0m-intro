use axum::extract::State;
use axum::handler::HandlerWithoutStateExt;
use axum::http::uri::Authority;
use axum::http::Uri;
use axum::response::{Redirect, Response};
use axum::routing::post;
use axum::Router;
use axum::{BoxError, Json};
use axum_extra::extract::Host;
use axum_server::tls_rustls::RustlsConfig;
use core::panic;
use reqwest::StatusCode;
use signaling::client::Client;
use std::collections::HashMap;
use std::future::Future;
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
    http: u16,
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
        http: 7878,
        https: 3000,
    };

    // Create a handle for our TLS server so the shutdown signal can all shutdown
    let handle = axum_server::Handle::new();
    // save the future for easy shutting down of redirect server
    let shutdown_future = shutdown_signal(handle.clone());

    // optional: spawn a second server to redirect http requests to this server
    tokio::spawn(redirect_http_to_https(ports, shutdown_future));

    let (tx, rx): (Sender<Client>, mpsc::Receiver<Client>) = mpsc::channel(10);
    let state = AppState {
        client_channel: tx.clone(),
    };

    // Separate thread to process WHIP/WHEP clients
    tokio::spawn(process_clients(rx));

    // configure certificate and private key used by https
    let config = RustlsConfig::from_pem_file(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("self_signed_certs")
            .join("cert.pem"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("self_signed_certs")
            .join("key.pem"),
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
        {
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

async fn redirect_http_to_https<F>(ports: Ports, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    fn make_https(host: &str, uri: Uri, https_port: u16) -> Result<Uri, BoxError> {
        let mut parts = uri.into_parts();

        parts.scheme = Some(axum::http::uri::Scheme::HTTPS);

        if parts.path_and_query.is_none() {
            parts.path_and_query = Some("/".parse().unwrap());
        }

        let authority: Authority = host.parse()?;
        let bare_host = match authority.port() {
            Some(port_struct) => authority
                .as_str()
                .strip_suffix(port_struct.as_str())
                .unwrap()
                .strip_suffix(':')
                .unwrap(), // if authority.port() is Some(port) then we can be sure authority ends with :{port}
            None => authority.as_str(),
        };

        parts.authority = Some(format!("{bare_host}:{https_port}").parse()?);

        Ok(Uri::from_parts(parts)?)
    }

    let redirect = move |Host(host): Host, uri: Uri| async move {
        match make_https(&host, uri, ports.https) {
            Ok(uri) => Ok(Redirect::permanent(&uri.to_string())),
            Err(error) => {
                tracing::warn!(%error, "failed to convert URI to HTTPS");
                Err(StatusCode::BAD_REQUEST)
            }
        }
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], ports.http));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tracing::debug!("listening on {addr}");
    axum::serve(listener, redirect.into_make_service())
        .with_graceful_shutdown(signal)
        .await
        .unwrap();
}
