#[macro_use]
extern crate tracing;

use std::io::ErrorKind;
use std::net::UdpSocket;
use std::process;
use std::thread;
use std::time::Instant;

use rouille::Server;
use rouille::{Request, Response};

use str0m::change::SdpOffer;
use str0m::net::Protocol;
use str0m::net::Receive;
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcError};

mod util;

fn init_log() {
    use std::env;
    // use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    use tracing_subscriber::{fmt, prelude::*};

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "http_post=debug,str0m=debug");
    }

    tracing_subscriber::registry()
        .with(fmt::layer())
        // .with(EnvFilter::from_default_env())
        .init();
}

pub fn main() {
    init_log();

    let certificate = include_bytes!("cer.pem").to_vec();
    let private_key = include_bytes!("key.pem").to_vec();

    // Figure out some public IP address, since Firefox will not accept 127.0.0.1 for WebRTC traffic.
    let host_addr = util::select_host_address();

    let server = Server::new_ssl("0.0.0.0:3000", web_request, certificate, private_key)
        .expect("starting the web server");

    let port = server.server_addr().port();
    info!("Connect a browser to https://{:?}:{:?}", host_addr, port);

    server.run();
}

// Handle a web request.
fn web_request(request: &Request) -> Response {
    if request.method() == "GET" {
        return Response::html(include_str!("http-post.html"));
    }

    // Expected POST SDP Offers.
    let mut data = request.data().expect("body to be available");

    let offer: SdpOffer = serde_json::from_reader(&mut data).expect("serialized offer");
    let mut rtc = Rtc::new();

    let addr = util::select_host_address();

    // Spin up a UDP socket for the RTC
    let socket = UdpSocket::bind(format!("{addr}:0")).expect("binding a random UDP port");
    let addr = socket.local_addr().expect("a local socket adddress");
    let candidate = Candidate::host(addr, "udp").expect("a host candidate");
    rtc.add_local_candidate(candidate);

    // Create an SDP Answer.
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .expect("offer to be accepted");

    // Launch WebRTC in separate thread.
    thread::spawn(|| {
        if let Err(e) = run(rtc, socket) {
            eprintln!("Exited: {e:?}");
            process::exit(1);
        }
    });

    let body = serde_json::to_vec(&answer).expect("answer to serialize");

    Response::from_data("application/json", body)
}

fn run(mut rtc: Rtc, socket: UdpSocket) -> Result<(), RtcError> {
    // Buffer for incoming data.
    let mut buf = Vec::new();

    loop {
        // Poll output until we get a timeout. The timeout means we are either awaiting UDP socket input
        // or the timeout to happen.
        let timeout = match rtc.poll_output()? {
            Output::Timeout(v) => v,

            Output::Transmit(v) => {
                socket.send_to(&v.contents, v.destination)?;
                continue;
            }

            Output::Event(v) => {
                // Abort if we disconnect.
                if v == Event::IceConnectionStateChange(IceConnectionState::Disconnected) {
                    return Ok(());
                }

                // TODO: handle other events, such as incoming media data.

                continue;
            }
        };

        let timeout = timeout - Instant::now();

        // socket.set_read_timeout(Some(0)) is not ok
        if timeout.is_zero() {
            // Drive time forwards in rtc straight away.
            rtc.handle_input(Input::Timeout(Instant::now()))?;
            continue;
        }

        socket.set_read_timeout(Some(timeout))?;

        // Scale up buffer to receive an entire UDP packet.
        buf.resize(2000, 0);

        // Try to receive. Because we have a timeout on the socket,
        // we will either receive a packet, or timeout.
        // This is where having an async loop shines. We can await multiple things to
        // happen such as outgoing media data, the timeout and incoming network traffic.
        // When using async there is no need to set timeout on the socket.
        let input = match socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                // UDP data received.
                buf.truncate(n);
                Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: socket.local_addr().unwrap(),
                        contents: buf.as_slice().try_into()?,
                    },
                )
            }

            Err(e) => match e.kind() {
                // Expected error for set_read_timeout(). One for windows, one for the rest.
                ErrorKind::WouldBlock | ErrorKind::TimedOut => Input::Timeout(Instant::now()),
                _ => return Err(e.into()),
            },
        };

        rtc.handle_input(input)?;
    }
}