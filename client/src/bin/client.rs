use anyhow::Error;
use signaling::client::Client;
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::JoinSet,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().await.expect("Failed to create client");

    client.make_whip_request().await?;

    // * Channel for RTP packets
    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel(5);
    let mut set = JoinSet::new();    

    set.spawn_blocking(move || media::stream_test_video(tx.clone()));
    set.spawn(run_client_loop(client, rx));

    set.join_all().await;

    Ok(())
}

async fn run_client_loop(mut client: Client, mut rx: Receiver<Vec<u8>>) -> Result<(), Error> {
    loop {
        client.run().await?;
        client.send_video(&mut rx)?;
    }
}
