use anyhow::Error;
use signaling::client::Client;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = Client::new().expect("Failed to create client");

    client.make_whip_request().await?;

    Ok(())
}
