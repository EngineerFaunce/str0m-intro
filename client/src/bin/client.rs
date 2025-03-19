use anyhow::Error;
use signaling::{
    client::{Disconnected, RtcClient},
    message::SdpMessageType,
    WebRtcEvent,
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client: RtcClient<Disconnected> = RtcClient::new().expect("Failed to create client");

    let (sdp_message, client) = client.get_offer().await?;

    match sdp_message {
        SdpMessageType::SdpOffer(offer) => {
            // * Create an SDP Answer.
            let mut client = client.accept_offer(offer).await?;

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
        }
        SdpMessageType::SdpAnswer(_) => panic!("Expected an offer, but received an answer"),
    }

    Ok(())
}
