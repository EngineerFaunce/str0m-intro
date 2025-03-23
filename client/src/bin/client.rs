use anyhow::Error;
use signaling::{
    client::{Client, Disconnected},
    message::SdpMessageType,
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client: Client<Disconnected> = Client::new().expect("Failed to create client");

    let (sdp_message, client) = client.get_offer().await?;

    match sdp_message {
        SdpMessageType::SdpOffer(offer) => {
            // * Create an SDP Answer.
            let mut client = client.accept_offer(offer).await?;
        }
        SdpMessageType::SdpAnswer(_) => panic!("Expected an offer, but received an answer"),
    }

    Ok(())
}
