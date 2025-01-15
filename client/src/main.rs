use anyhow::Error;
use reqwest::{self, ClientBuilder};
use signaling::{
    client::Client,
    message::{SdpExchange, SdpMessageType},
    util::{logging::init_log, network::get_host_ip_address},
    WebRtcEvent,
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init_log();

    // TODO (future): Will likely need to be updated to accept input of the server's address
    let base_url = format!("https://{}:3000", get_host_ip_address());

    let http_client = ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .build()?;

    // * Make a GET request to the server to get the offer.
    let signal_url = format!("{}/offer", base_url);
    let res = http_client.get(signal_url).send().await?;

    // Deserialize the client ID and SdpOffer.
    let exchange = res
        .json::<SdpExchange>()
        .await
        .expect("offer to be deserialized");
    let client_id = exchange.client_id;
    let sdp_message = exchange.sdp_payload;

    let mut client = Client::new().expect("Failed to create client");

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
