use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use core::panic;
use reqwest::StatusCode;
use signaling::client::{Client, Connected, Pending};
use signaling::message::{SdpExchange, SdpMessageType};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::{io::Read, thread};
use str0m::change::SdpAnswer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

enum SignalMessage {
    Offer(Client<Pending>),
    Answer(AnswerSignal),
}

struct AnswerSignal {
    id: Uuid,
    answer: SdpAnswer,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    // let certificate = include_bytes!("../certs/cer.pem").to_vec();
    // let private_key = include_bytes!("../certs/key.pem").to_vec();

    // ? tx = transmission
    // ? rx = receiving
    let (tx, rx): (SyncSender<SignalMessage>, Receiver<SignalMessage>) = mpsc::sync_channel(1);

    // Separate thread to process clients as offers are made/accepted.
    thread::spawn(move || run(rx));

    let app = Router::new().route("/health", get(health));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("Listening on: {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();

    // let server = Server::new_ssl(
    //     "0.0.0.0:3000",
    //     move |request| web_request(request, tx.clone()),
    //     certificate,
    //     private_key,
    // )
    // .expect("starting the web server");

    // server.run();
}

async fn health() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

// Handle a web request.
// fn web_request(request: &Request, tx: SyncSender<SignalMessage>) -> Response {
//     // ? This is just for debugging purposes.
//     if request.url() == "/health" && request.method() == "GET" {
//         info!("Received request from: {:?}", request.remote_addr());
//         return Response::empty_204();
//     }

//     // * This is one half of the signaling process where we create an offer and send it to the client.
//     if request.url() == "/offer" && request.method() == "GET" {
//         let client = Client::new().expect("Failed to create client");

//         let (offer, client) = client.create_offer().expect("offer to be created");

//         let response = SdpExchange {
//             client_id: client.id,
//             sdp_payload: SdpMessageType::SdpOffer(offer),
//         };

//         tx.send(SignalMessage::Offer(client))
//             .expect("client to be sent");

//         return Response::json(&response);
//     }

//     // * This is the other half of the signaling process. The client sends an answer back and we accept it.
//     if request.url() == "/answer" && request.method() == "POST" {
//         // Deserialize the answer.
//         let mut body = request.data().expect("body to be available");
//         let mut buf = Vec::new();
//         body.read_to_end(&mut buf).expect("data to be read");
//         let exchange: SdpExchange = serde_json::from_slice(&buf).expect("data to be deserialized");

//         match exchange.sdp_payload {
//             SdpMessageType::SdpOffer(_) => panic!("Expected an answer, but got an offer."),
//             SdpMessageType::SdpAnswer(answer) => {
//                 let answer = AnswerSignal {
//                     id: exchange.client_id,
//                     answer,
//                 };
//                 tx.send(SignalMessage::Answer(answer))
//                     .expect("answer to be sent");
//             }
//         }

//         return Response::empty_204();
//     }
//     Response::empty_404()
// }

fn run(rx: Receiver<SignalMessage>) {
    let mut pending_clients: HashMap<Uuid, Client<Pending>> = HashMap::new();
    let mut clients: HashMap<Uuid, Client<Connected>> = HashMap::new();

    loop {
        // Remove disconnected clients.
        clients.retain(|_, c| c.rtc.is_alive());

        match rx.try_recv() {
            Ok(SignalMessage::Offer(client)) => {
                info!("Sent offer to client: {:?}", client.id);
                pending_clients.insert(client.id, client);
            }
            Ok(SignalMessage::Answer(answer)) => {
                info!("Received answer from client: {:?}", answer.id);

                // Accept the answer
                // TODO: error handling
                let client = pending_clients.remove(&answer.id).unwrap();
                let client = client
                    .accept_answer(answer.answer)
                    .expect("answer to be accepted");
                clients.insert(client.id, client);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                panic!("Channel disconnected");
            }
        };

        // TODO: start polling clients
        // TODO: propagate changes to other clients
    }
}
