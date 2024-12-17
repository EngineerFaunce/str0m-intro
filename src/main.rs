#[macro_use]
extern crate tracing;

use std::io::Read;
use client::Client;
use rouille::{Server, Request, Response};

use str0m::change::SdpAnswer;
use str0m_intro::{get_external_ip_address, init_log};

mod client;


pub fn main() {
    init_log();

    let certificate = include_bytes!("cer.pem").to_vec();
    let private_key = include_bytes!("key.pem").to_vec();

    // Figure out some public IP address, since Firefox will not accept 127.0.0.1 for WebRTC traffic.
    let host_addr = get_external_ip_address();

    let server = Server::new_ssl("0.0.0.0:3000", web_request, certificate, private_key)
        .expect("starting the web server");

    let port = server.server_addr().port();
    info!("Connect a browser to https://{:?}:{:?}", host_addr, port);

    server.run();
}

// Handle a web request.
fn web_request(request: &Request) -> Response {
    // ! A simple client implementation.
    if request.url() == "/" && request.method() == "GET" {
        return Response::html(include_str!("http-post.html"));
    }

    // ? This is just for debugging purposes.
    if request.url() == "/health" && request.method() == "GET" {
        info!("Received request from: {:?}", request.remote_addr());
        return Response::empty_204();
    }

    let mut client = Client::new().expect("Failed to create client");

    // * This is one half of the signaling process where we create an offer and send it to the client.
    if request.url() == "/offer" && request.method() == "GET" {
        client.add_local_candidate(request.remote_addr());
        let offer = client.create_offer().expect("offer to be created");
        return Response::json(&offer);
    }

    // * This is the other half of the signaling process. The client sends an answer back and we accept it.
    if request.url() == "/answer" && request.method() == "POST" {
        let mut data = request.data().expect("body to be available");
        let mut buf = Vec::new();
        data.read_to_end(&mut buf).expect("data to be read");
        let answer: SdpAnswer = serde_json::from_slice(&buf).expect("data to be deserialized");
        
        // TODO: accept answer from client.
        client.accept_offer(answer).expect("answer to be accepted");
    }
    Response::empty_404()
}
