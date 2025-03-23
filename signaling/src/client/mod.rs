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
pub struct Disconnected;
pub struct Pending;
pub struct Connected;

#[derive(Debug)]
pub struct Client<ConnectionState = Disconnected> {
    pub id: Uuid,
    rtc: Rtc,
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

    pub async fn get_offer(self) -> Result<(SdpMessageType, Client<Pending>), Error> {
        // TODO (future): Will likely need to be updated to accept input of the server's address
        let base_url = format!("https://{}:3000", get_host_ip_address());

        // * Make a GET request to the server to get the offer.
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
    pub fn recv(&mut self) -> Result<WebRtcEvent, RtcError> {
        if !self.rtc.is_alive() {
            return Ok(WebRtcEvent::Disconnected);
        }

        // Poll output until we get a timeout. The timeout means we are either awaiting UDP socket input
        // or the timeout to happen.
        let timeout = match self.rtc.poll_output()? {
            Output::Event(event) => match event {
                Event::Connected => {
                    info!("connected");
                    return Ok(WebRtcEvent::Continue);
                }
                Event::IceConnectionStateChange(state) => {
                    info!("ice connection state change: {:?}", state);
                    return Ok(WebRtcEvent::Continue);
                }
                // TODO: handle other events, such as incoming media data.
                _ => {
                    return Ok(WebRtcEvent::Continue);
                }
            },
            Output::Timeout(timeout) => timeout,
            Output::Transmit(send) => {
                self.socket.send_to(&send.contents, send.destination)?;
                return Ok(WebRtcEvent::Continue);
            }
        };

        let duration = timeout - Instant::now();

        if duration.is_zero() {
            // Drive time forwards in rtc straight away.
            self.rtc.handle_input(Input::Timeout(Instant::now()))?;
            return Ok(WebRtcEvent::Continue);
        }

        self.socket.set_read_timeout(Some(duration))?;

        let mut buf = vec![0; 1500];
        let input = match self.socket.recv_from(&mut buf) {
            Ok((n, source)) => {
                // UDP data received.
                buf.truncate(n);
                Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: self.socket.local_addr().unwrap(),
                        contents: buf.as_slice().try_into()?,
                    },
                )
            }

            Err(e) => match e.kind() {
                // Expected error for set_read_timeout(). One for windows, one for the rest.
                ErrorKind::WouldBlock | ErrorKind::TimedOut => Input::Timeout(Instant::now()),
                _ => return Err(e.into()),
            },
        };

        self.rtc.handle_input(input)?;

        return Ok(WebRtcEvent::Continue);
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
