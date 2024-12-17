use reqwest::{self, ClientBuilder};
use str0m::change::SdpOffer;
use str0m_intro::{client::Client, logging::init_log, util::get_external_ip_address};
use tokio;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    init_log();

    let base_url = format!("https://{}:3000", get_external_ip_address());

    let http_client = ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .build()?;
    
    // * Make a GET request to the server to get the offer.
    let signal_url = format!("{}/offer", base_url);
    let res = http_client.get(signal_url)
        .send()
        .await?;

    // Deserialize the offer.
    let offer = res.json::<SdpOffer>().await.expect("offer to be deserialized");

    // * Create an SDP Answer.
    let mut client = Client::new().expect("Failed to create client");
    let answer = client.create_answer(offer).expect("answer to be created");

    // * Send the answer back to the server
    let answer_url = format!("{}/answer", base_url);
    let _ = http_client.post(answer_url)
        .json(&answer)
        .send()
        .await?;

    Ok(())
}
