use anyhow::Error;
use rtc::Client;
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
    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel(5);
    let token = CancellationToken::new();
    let mut set = JoinSet::new();

    set.spawn_blocking(move || media::stream_test_video(tx));

    let client_token = token.clone();
    set.spawn(run_client_loop(client, rx, client_token));

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

async fn run_client_loop(
    mut client: Client,
    mut rx: Receiver<Vec<u8>>,
    token: CancellationToken,
) -> Result<(), Error> {
    loop {
        tokio::select! {
            // res = client.run(token.clone()) => {
            //     res?;
            // }
            _ = token.cancelled() => break,
        }

        tokio::select! {
            res = async {
                client.send_video(&mut rx).map_err(Error::from)
            } => {
                res?;
            }
            _ = token.cancelled() => break,
        }
    }

    Ok(())
}
