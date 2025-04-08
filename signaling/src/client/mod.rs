use reqwest::header::{HeaderValue, ACCEPT};
use reqwest::{header::CONTENT_TYPE, ClientBuilder};
use std::{
    marker::PhantomData,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};
use str0m::{
    change::{SdpAnswer, SdpOffer},
    Candidate, Rtc, RtcError,
};
use tracing::debug;
use uuid::Uuid;

/// The states of the Rtc client
/// Initial - The client has been created but no offer has been created
/// Connected - The client has received an answer and is connected
// TODO: don't do this. It can be done without the need for separate structs
pub struct Disconnected;
pub struct Connected;

#[derive(Debug)]
pub struct Client<ConnectionState = Disconnected> {
    pub id: Uuid,
    pub rtc: Rtc,
    socket: UdpSocket,
    state: PhantomData<ConnectionState>,
}

impl<T> Client<T> {
    fn transition<State>(self) -> Client<State> {
        Client::<State> {
            id: self.id,
            rtc: self.rtc,
            socket: self.socket,
            state: PhantomData,
        }
    }
}

impl Client<Disconnected> {
    pub async fn make_whip_request(mut self) -> Result<(), reqwest::Error> {
        // WHIP client creates the offer
        let mut change = self.rtc.sdp_api();
        let _mid = change.add_media(
            str0m::media::MediaKind::Video,
            str0m::media::Direction::SendOnly, // The offer *should* use the sendonly attribute
            None,
            None,
        );
        let (offer, pending) = change.apply().unwrap();

        // Set some default headers based on WHIP protocol
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            CONTENT_TYPE,
            HeaderValue::from_str("application/sdp").unwrap(),
        );
        headers.append(ACCEPT, HeaderValue::from_str("application/sdp").unwrap());

        let http_client = ClientBuilder::new()
            .default_headers(headers)
            .build()
            .unwrap();

        // TODO (future): Will likely need to be updated to accept input of the server's address
        let base_url = "https://127.0.0.1:3000";
        let signal_url = format!("{}/whip", base_url);

        // WHIP client makes a POST request to the WHIP endpoint
        // WHIP endpoint responds with a 201 and SDP answer in the body
        let answer_string = http_client
            .post(signal_url)
            .json(&offer)
            .send()
            .await?
            .text()
            .await?;

        let answer = SdpAnswer::from_sdp_string(answer_string.as_str()).unwrap();

        self.rtc.sdp_api().accept_answer(pending, answer).unwrap();

        Ok(())
    }

    pub async fn accept_whip_request(
        mut self,
        offer: SdpOffer,
    ) -> Result<(Client<Connected>, String), RtcError> {
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        Ok((self.transition(), answer.to_sdp_string()))
    }
}

impl Client<Connected> {
    // TODO: methods for ingress and egress
    // TODO: method for disconnecting
}

impl Client {
    pub fn new() -> Result<Self, RtcError> {
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
        debug!("local socket address: {:?}", socket.local_addr());

        rtc.add_local_candidate(
            Candidate::host(socket_addr, str0m::net::Protocol::Udp)
                .expect("Failed to create local candidate"),
        );

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            state: PhantomData,
            rtc,
            socket,
        })
    }
}
