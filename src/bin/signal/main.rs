#[macro_use]
extern crate tracing;

use core::panic;
use rouille::{Request, Response, Server};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::{io::Read, thread};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m_intro::client::{Client, WebRtcEvent};
use str0m_intro::util::logging::init_log;
use str0m_intro::util::{SdpExchange, SdpMessageType};
use uuid::Uuid;

enum Signal {
    Offer(Client),
    Answer(AnswerSignal),
}

struct AnswerSignal {
    id: Uuid,
    answer: SdpAnswer,
}

pub fn main() {
    init_log();

    let certificate = include_bytes!("../../certs/cer.pem").to_vec();
    let private_key = include_bytes!("../../certs/key.pem").to_vec();

    // // Figure out some public IP address, since Firefox will not accept 127.0.0.1 for WebRTC traffic.
    // let host_addr = get_host_ip_address();

    // ? tx = transmission
    // ? rx = receiving
    let (tx, rx): (SyncSender<Signal>, Receiver<Signal>) = mpsc::sync_channel(1);

    // Separate thread to process clients as offers are made/accepted.
    thread::spawn(move || process_clients(rx));

    let server = Server::new_ssl(
        "0.0.0.0:3000",
        move |request| web_request(request, tx.clone()),
        certificate,
        private_key,
    )
    .expect("starting the web server");

    // let port = server.server_addr().port();
    // info!("Connect a browser to https://{:?}:{:?}", host_addr, port);

    server.run();
}

// Handle a web request.
fn web_request(request: &Request, tx: SyncSender<Signal>) -> Response {
    // ? This is just for debugging purposes.
    if request.url() == "/health" && request.method() == "GET" {
        info!("Received request from: {:?}", request.remote_addr());
        return Response::empty_204();
    }

    // * This is one half of the signaling process where we create an offer and send it to the client.
    if request.url() == "/offer" && request.method() == "GET" {
        let mut client = Client::new().expect("Failed to create client");
        // client.add_local_candidate(&addr);
        let offer: SdpOffer = client.create_offer().expect("offer to be created");

        let response = SdpExchange {
            client_id: client.id,
            sdp_payload: str0m_intro::util::SdpMessageType::SdpOffer(offer),
        };

        tx.send(Signal::Offer(client)).expect("client to be sent");

        return Response::json(&response);
    }

    // * This is the other half of the signaling process. The client sends an answer back and we accept it.
    if request.url() == "/answer" && request.method() == "POST" {
        // Deserialize the answer.
        let mut body = request.data().expect("body to be available");
        let mut buf = Vec::new();
        body.read_to_end(&mut buf).expect("data to be read");
        let exchange: SdpExchange = serde_json::from_slice(&buf).expect("data to be deserialized");

        match exchange.sdp_payload {
            SdpMessageType::SdpOffer(_) => panic!("Expected an answer"),
            SdpMessageType::SdpAnswer(answer) => {
                let answer = AnswerSignal {
                    id: exchange.client_id,
                    answer,
                };
                tx.send(Signal::Answer(answer)).expect("answer to be sent");
            }
        }

        return Response::empty_204();
    }
    Response::empty_404()
}

fn process_clients(rx: Receiver<Signal>) {
    let mut pending_clients: HashMap<Uuid, Client> = HashMap::new();

    loop {
        match rx.try_recv() {
            Ok(Signal::Offer(client)) => {
                info!("Sent offer to client: {:?}", client.id);
                pending_clients.insert(client.id, client);
            }
            Ok(Signal::Answer(answer)) => {
                info!("Received answer from client: {:?}", answer.id);
                // Accept the answer
                let mut client = pending_clients.remove(&answer.id).unwrap();
                client
                    .accept_answer(answer.answer)
                    .expect("answer to be accepted");

                // Start polling the client for incoming data.
                thread::spawn(move || loop {
                    let event = client.recv();
                    match event {
                        Ok(WebRtcEvent::Continue) => {}
                        Ok(WebRtcEvent::Disconnected) => {
                            break;
                        }
                        Err(_e) => {
                            break;
                        }
                    }
                });
            }
            Err(TryRecvError::Empty) => {
                // info!("No client received");
            }
            Err(TryRecvError::Disconnected) => {
                info!("Channel disconnected");
            }
        }
    }
}
