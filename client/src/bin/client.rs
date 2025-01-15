use anyhow::Error;
// TODO: this is silly. Should this be declared in lib.rs?
use client::client::Client;
use reqwest::{self, ClientBuilder};
use signaling::{
    message::{SdpExchange, SdpMessageType},
    util::{logging::init_log, network::get_host_ip_address},
    WebRtcEvent,
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init_log();

    let mut client = Client::new().expect("Failed to create client");

    let sdp_message = client.send_offer().await?;

    match sdp_message {
        SdpMessageType::SdpOffer(offer) => {
            // * Create an SDP Answer.
            let answer = client.create_answer(offer).expect("answer to be created");

            // * Send the answer back to the server
            let answer = SdpExchange {
                client_id,
                sdp_payload: SdpMessageType::SdpAnswer(answer),
            };
            let answer_url = format!("{}/answer", base_url);
            let _ = http_client.post(answer_url).json(&answer).send().await?;
        }
        SdpMessageType::SdpAnswer(_) => panic!("Expected an offer"),
    }

    // Start polling for input.
    loop {
        let event = client.recv();
        match event {
            Ok(WebRtcEvent::Continue) => {
                continue;
            }
            Ok(WebRtcEvent::Disconnected) => {
                info!("disconnected");
                break;
            }
            Err(_e) => {
                info!("error {:?}", _e);
                break;
            }
        }
    }

    Ok(())
}
