use anyhow::Error;
use signaling::client::Client;
use std::sync::mpsc::{self, Receiver, Sender};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().await.expect("Failed to create client");

    client.make_whip_request().await?;

    // * Channel for RTP packets
    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();
    tokio::task::spawn_blocking(move || media::stream_test_video(tx.clone()));

    loop {
        if let Err(e) = client.run().await {
            eprintln!("Error running client: {:?}", e);
            break;
        }
        client.send_video(&rx)?;
    }

    Ok(())
}
