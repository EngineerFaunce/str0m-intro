use anyhow::Error;
use signaling::{client::Client, message::SdpMessageType, util::logging::init_log, WebRtcEvent};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init_log();

    let mut client = Client::new().expect("Failed to create client");

    let sdp_message = client.send_offer().await?;

    match sdp_message {
        SdpMessageType::SdpOffer(offer) => {
            // * Create an SDP Answer.
            client.create_answer(offer).expect("answer to be created");
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
