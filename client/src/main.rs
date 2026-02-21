use anyhow::Error;
use rtc::{Client, OutboundRtpPacket};
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let mut client = Client::new().await.expect("Failed to create client");

    client.make_whip_request().await?;

    // * Channel for RTP packets
    let (tx, _rx): (Sender<OutboundRtpPacket>, Receiver<OutboundRtpPacket>) = mpsc::channel(100);
    let token = CancellationToken::new();
    let mut set = JoinSet::new();

    set.spawn(media::stream_test_video(tx));
    // TODO: handle the other work needed;
    // - Sending the RTP packets to the session (SFU)
    // - Driving the state of the client

    let mut failure: Option<Error> = None;
    while let Some(result) = set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                token.cancel();
                failure.get_or_insert(e);
                break;
            }
            Err(join_error) => {
                token.cancel();
                failure.get_or_insert(join_error.into());
                break;
            }
        }
    }

    if let Some(err) = failure {
        while set.join_next().await.is_some() {}
        return Err(err);
    }

    Ok(())
}
