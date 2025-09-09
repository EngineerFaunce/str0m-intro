use anyhow::Error;
use signaling::client::Client;
use tracing::debug;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().expect("Failed to create client");

    client.make_whip_request().await?;

    tokio::spawn(async move {
        loop {
            match client.run() {
                Ok(_) => {}
                Err(e) => {
                    debug!("Client ran into error: {:?}", e);
                    continue;
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });

    let _ = client.stream_test_video();

    Ok(())
}
