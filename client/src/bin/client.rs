use anyhow::Error;
use signaling::client::{Client, Disconnected};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client: Client<Disconnected> = Client::new().expect("Failed to create client");

    Ok(())
}
