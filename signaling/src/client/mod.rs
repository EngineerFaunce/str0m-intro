use crate::{
    message::{SdpExchange, SdpMessageType},
    util::network::get_host_ip_address,
    WebRtcEvent,
};
use anyhow::Error;
use core::panic;
use reqwest::ClientBuilder;
use std::{
    io::ErrorKind,
    marker::PhantomData,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};
use str0m::{
    change::{SdpAnswer, SdpOffer, SdpPendingOffer},
    net::{Protocol, Receive},
    Candidate, Event, Input, Output, Rtc, RtcError,
};
use tracing::info;
use uuid::Uuid;

/// The states of the Rtc client
/// Initial - The client has been created but no offer has been created
/// Pending - The client has created an offer and is waiting for a response
/// Connected - The client has received an answer and is connected
// TODO: don't do this. It can be done without the need for separate structs
pub struct Disconnected;
pub struct Pending;
pub struct Connected;

#[derive(Debug)]
pub struct Client<ConnectionState = Disconnected> {
    pub id: Uuid,
    pub rtc: Rtc,
    socket: UdpSocket,
    pending: Option<SdpPendingOffer>,
    http_client: reqwest::Client,
    state: PhantomData<ConnectionState>,
}

impl<T> Client<T> {
    fn transition<State>(self) -> Client<State> {
        Client::<State> {
            id: self.id,
            rtc: self.rtc,
            socket: self.socket,
            pending: self.pending,
            http_client: self.http_client,
            state: PhantomData,
        }
    }
}

impl Client<Disconnected> {
    /// Create an SdpOffer and return the client in the Pending state.
    pub fn create_offer(mut self) -> Result<(SdpOffer, Client<Pending>), RtcError> {
        let mut change = self.rtc.sdp_api();
        let _mid = change.add_media(
            str0m::media::MediaKind::Video,
            str0m::media::Direction::SendRecv,
            None,
            None,
        );
        let (offer, pending) = change.apply().unwrap();

        Ok((offer, self.transition()))
    }

    /// Make a GET request to the server to receive an offer.
    pub async fn get_offer(self) -> Result<(SdpMessageType, Client<Pending>), Error> {
        // TODO (future): Will likely need to be updated to accept input of the server's address
        let base_url = format!("https://{}:3000", get_host_ip_address());

        let signal_url = format!("{}/offer", base_url);
        let res = self.http_client.get(signal_url).send().await?;

        // Deserialize the client ID and SdpOffer.
        let exchange = res
            .json::<SdpExchange>()
            .await
            .expect("offer to be deserialized");

        // TODO: log the client ID?
        // let client_id = exchange.client_id;
        let sdp_message = exchange.sdp_payload;

        Ok((sdp_message, self.transition()))
    }
}

impl Client<Pending> {
    pub async fn accept_offer(mut self, offer: SdpOffer) -> Result<Client<Connected>, Error> {
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        let base_url = format!("https://{}:3000", get_host_ip_address());

        let answer_url = format!("{}/answer", base_url);
        let exchange = SdpExchange {
            client_id: self.id,
            sdp_payload: SdpMessageType::SdpAnswer(answer),
        };

        let res = self
            .http_client
            .post(answer_url)
            .json(&exchange)
            .send()
            .await?;

        Ok(self.transition())
    }

    pub fn accept_answer(mut self, answer: SdpAnswer) -> Result<Client<Connected>, RtcError> {
        let _ = self
            .rtc
            .sdp_api()
            .accept_answer(self.pending.take().unwrap(), answer);

        Ok(self.transition())
    }
}

impl Client<Connected> {
    // TODO: break this up like in the chat example
    pub fn recv(&mut self) -> Result<WebRtcEvent, RtcError> {
        todo!()
    }
}

impl Client {
    pub fn new() -> Result<Self, RtcError> {
        // * Set up the http client
        let http_client = match ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .build()
        {
            Ok(client) => client,
            // TODO: handle this error more gracefully
            Err(e) => panic!("Failed to create http client: {:?}", e),
        };

        // * Set up the WebRTC client
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_h264(true)
            .set_stats_interval(Some(Duration::from_secs(2)))
            .set_reordering_size_video(1)
            .set_reordering_size_audio(1)
            .build();

        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let socket = UdpSocket::bind(socket_addr).expect("Should bind udp socket");
        // debug!("local socket address: {:?}", socket.local_addr());

        rtc.add_local_candidate(
            Candidate::host(socket_addr, str0m::net::Protocol::Udp)
                .expect("Failed to create local candidate"),
        );

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            state: PhantomData,
            rtc,
            socket,
            pending: None,
            http_client,
        })
    }
}
