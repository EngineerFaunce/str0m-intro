use anyhow::Error;
use anyhow::Result;
use reqwest::header::{ACCEPT, HeaderValue};
use reqwest::{ClientBuilder, header::CONTENT_TYPE};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use str0m::Candidate;
use str0m::Event;
use str0m::IceConnectionState;
use str0m::Input;
use str0m::Output;
use str0m::media::Mid;
use str0m::net::Protocol;
use str0m::net::Receive;
use str0m::rtp::ExtensionValues;
use str0m::rtp::RtpPacket;
use str0m::{
    Rtc, RtcError,
    change::{SdpAnswer, SdpOffer},
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tracing::error;
use tracing::info;
use tracing::trace;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug)]
pub struct Client {
    pub id: Uuid,
    pub rtc: Rtc,
    video_mid: Option<Mid>,
}

impl Client {
    /// Creates a new WebRTC client
    pub async fn new() -> Result<Self, RtcError> {
        let rtc = Rtc::builder()
            .set_rtp_mode(true)
            .clear_codecs()
            .enable_h264(true)
            .set_stats_interval(Some(Duration::from_secs(2)))
            .build(Instant::now());

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            rtc,
            video_mid: None,
        })
    }

    /// Make a request to the WHIP endpoint
    pub async fn make_whip_request(&mut self) -> Result<(), Error> {
        // WHIP client creates the offer
        let mut change = self.rtc.sdp_api();
        self.video_mid = Some(change.add_media(
            str0m::media::MediaKind::Video,
            str0m::media::Direction::SendOnly, // The offer *should* use the sendonly attribute
            None,
            None,
            None,
        ));
        let (offer, pending) = change.apply().unwrap();

        // Set some default headers based on WHIP protocol
        let mut headers = reqwest::header::HeaderMap::new();
        let header_value = HeaderValue::from_str("application/sdp").unwrap();
        headers.append(CONTENT_TYPE, header_value.clone());
        headers.append(ACCEPT, header_value);

        let mut buf = Vec::new();

        // TODO: should the certificate and key be moved to a more central location?
        let temp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("server")
            .join("self_signed_certs")
            .join("cert.pem");
        let mut file = File::open(temp).await?;
        let _bytes_read = file.read_to_end(&mut buf).await?;
        let cert = reqwest::Certificate::from_pem(&buf)?;

        let http_client = ClientBuilder::new()
            .default_headers(headers)
            .add_root_certificate(cert)
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

    /// Accept an SdpOffer
    pub async fn accept_request(&mut self, offer: SdpOffer) -> Result<String, RtcError> {
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        Ok(answer.to_sdp_string())
    }

    /// Make a request to the WHEP endpoint
    pub async fn make_whep_request(&mut self) -> Result<(), Error> {
        // WHEP client creates the offer
        let mut change = self.rtc.sdp_api();
        self.video_mid = Some(change.add_media(
            str0m::media::MediaKind::Video,
            str0m::media::Direction::RecvOnly, // The offer *should* use the recvonly attribute
            None,
            None,
            None,
        ));
        let (offer, pending) = change.apply().unwrap();

        // Set some default headers based on WHEP protocol
        let mut headers = reqwest::header::HeaderMap::new();
        let header_value = HeaderValue::from_str("application/sdp").unwrap();
        headers.append(CONTENT_TYPE, header_value.clone());
        headers.append(ACCEPT, header_value);

        let mut buf = Vec::new();

        // TODO: should the certificate and key be moved to a more central location?
        let temp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("server")
            .join("self_signed_certs")
            .join("cert.pem");
        let mut file = File::open(temp).await?;
        let _bytes_read = file.read_to_end(&mut buf).await?;
        let cert = reqwest::Certificate::from_pem(&buf)?;

        let http_client = ClientBuilder::new()
            .default_headers(headers)
            .add_root_certificate(cert)
            .build()
            .unwrap();

        // TODO (future): Will likely need to be updated to accept input of the server's address
        let base_url = "https://127.0.0.1:3000";
        let signal_url = format!("{}/whep", base_url);

        // WHEP client makes a POST request to the WHEP endpoint
        // WHEP endpoint responds with a 201 and SDP answer in the body
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

    /// Polls the client until we receive a timeout.
    /// If a queue was passed, we push non-timeout events onto it
    pub async fn poll_until_timeout(
        &mut self,
        socket: &UdpSocket,
        mut queue: Option<&mut VecDeque<Propagated>>,
    ) -> Instant {
        loop {
            if !self.rtc.is_alive() {
                return Instant::now();
            }

            let propagated = self.poll_output(socket).await;
            // trace!("Propagated: {:?}", propagated);

            if let Propagated::Timeout(t) = propagated {
                return t;
            }

            if let Some(q) = queue.as_deref_mut() {
                q.push_back(propagated);
            }
        }
    }

    async fn poll_output(&mut self, socket: &UdpSocket) -> Propagated {
        if !self.rtc.is_alive() {
            trace!("Rtc instance not alive. Returning no-op.");
            return Propagated::Noop;
        }

        match self.rtc.poll_output() {
            Ok(output) => self.handle_output(socket, output).await,
            Err(e) => {
                warn!("Client ({}) poll_output failed: {:?}", &self.id, e);
                self.rtc.disconnect();
                Propagated::Noop
            }
        }
    }

    async fn handle_output(&mut self, socket: &UdpSocket, output: Output) -> Propagated {
        match output {
            // * Stop polling when we get a timeout
            Output::Timeout(timeout) => Propagated::Timeout(timeout),

            // * Transmit this data to the remote peer
            Output::Transmit(transmit) => {
                if let Err(e) = socket
                    .send_to(&transmit.contents, transmit.destination)
                    .await
                {
                    warn!(
                        "sending to {} => {}, len {} error {:?}",
                        transmit.source,
                        transmit.destination,
                        transmit.contents.len(),
                        e
                    );
                };
                Propagated::Noop
            }
            Output::Event(event) => match event {
                Event::Connected => {
                    info!("ICE connected and established DTLS.");
                    Propagated::Noop
                }
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    info!("ICE disconnected");
                    self.rtc.disconnect();
                    Propagated::Noop
                }
                Event::MediaAdded(media) => {
                    trace!("Media direction: {}", media.direction);
                    trace!("Codec config: {:?}", self.rtc.codec_config());
                    Propagated::Noop
                }
                Event::RtpPacket(packet) => {
                    info!("RTP packet event received: {:?}", packet);
                    Propagated::RtpPacket(self.id, packet)
                }
                Event::MediaData(_) => {
                    info!("Incoming media data from remote peer.");
                    Propagated::Noop
                }
                Event::MediaChanged(_) => {
                    info!("Media changed.");
                    Propagated::Noop
                }
                _ => {
                    warn!("Unhandled event.");
                    Propagated::Noop
                }
            },
        }
    }

    // TODO: better name?
    pub async fn recv(&mut self, socket: &UdpSocket) -> Result<WebRtcEvent, Error> {
        let timeout = match self.poll_output(&socket).await {
            Propagated::Noop => {
                info!("no-op event, continuing");
                return Ok(WebRtcEvent::Continue);
            }
            Propagated::Timeout(timeout) => timeout,
            Propagated::RtpPacket(_, _) => {
                panic!("publisher received RTP packet")
            }
        };

        let duration = (timeout - Instant::now()).max(Duration::from_millis(20));
        trace!("Timeout duration: {:?}", duration);

        let mut buf = vec![0; 2000];
        match tokio::time::timeout(duration, socket.recv_from(&mut buf)).await {
            Ok(result) => {
                let input = match result {
                    Ok((n, source)) => {
                        buf.truncate(n);

                        // Parse data to a DatagramRecv
                        let Ok(contents) = buf.as_slice().try_into() else {
                            panic!("idk")
                        };

                        Input::Receive(
                            Instant::now(),
                            Receive {
                                proto: Protocol::Udp,
                                source,
                                destination: socket.local_addr().unwrap(),
                                contents,
                            },
                        )
                    }

                    Err(e) => match e.kind() {
                        // Expected error for set_read_timeout(). One for windows, one for the rest.
                        ErrorKind::WouldBlock | ErrorKind::TimedOut => {
                            Input::Timeout(Instant::now())
                        }
                        _ => panic!("UdpSocket read failed: {e:?}"),
                    },
                };

                // TODO: handle errors
                match self.rtc.handle_input(input) {
                    Ok(_) => {}
                    Err(e) => {
                        panic!("unhandled RtcError: {:?}", e)
                    }
                }
            }
            Err(_) => {
                // error!("Duration has elapsed before future could complete.");
            }
        }

        match self.rtc.handle_input(Input::Timeout(Instant::now())) {
            Ok(()) => trace!("driving state of publisher: {}", self.id),
            Err(e) => {
                error!("Unhandled error: {:?}", e);
            }
        }

        Ok(WebRtcEvent::Continue)
    }

    pub fn accepts(&self, input: &Input) -> bool {
        self.rtc.accepts(input)
    }

    pub fn handle_input(&mut self, input: Input) {
        if !self.rtc.is_alive() {
            return;
        }

        if let Err(e) = self.rtc.handle_input(input) {
            tracing::warn!("Client ({}) disconnected: {:?}", self.id, e);
            self.rtc.disconnect();
        }
    }

    pub fn write_rtp_packet<P>(&mut self, packet: P)
    where
        OutboundRtpPacket: From<P>,
    {
        let packet = OutboundRtpPacket::from(packet);

        // * Acquire a send stream and write the RTP packet
        let mut direct_api = self.rtc.direct_api();
        let stream_tx = direct_api
            .stream_tx_by_mid(self.video_mid.unwrap(), None)
            .unwrap();
        match stream_tx.write_rtp(
            packet.payload_type.into(),
            (packet.sequence_number as u64).into(),
            packet.timestamp.into(),
            Instant::now(),
            packet.marker, // marker
            ExtensionValues::default(),
            false, // not padding
            packet.payload,
        ) {
            Ok(_) => {
                tracing::debug!(
                    "Sent RTP packet: seq={:?}, ts={}, pt={}",
                    packet.sequence_number,
                    packet.timestamp,
                    packet.payload_type
                );
            }
            // TODO: handle specific PacketError cases
            Err(e) => {
                tracing::error!("Failed to send RTP packet: {:?}", e);
            }
        }
    }

    /// Wrapper function for adding a local candidate
    pub fn add_local_candidate(&mut self, socket_addr: SocketAddr) {
        let candidate = Candidate::host(socket_addr, Protocol::Udp).unwrap();
        self.rtc.add_local_candidate(candidate);
    }
}

// Possible events for publisher RTC client
#[derive(Debug)]
pub enum WebRtcEvent {
    Continue,
    Disconnected,
    RtpPacket(RtpPacket),
}

#[derive(Debug)]
pub enum Propagated {
    /// Nothing to propagate
    Noop,

    /// Poll client has reached timeout
    Timeout(Instant),

    /// RTP packet to be propagated from one client to others
    RtpPacket(Uuid, RtpPacket),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundRtpPacket {
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub marker: bool,
    pub payload: Vec<u8>,
}

impl From<RtpPacket> for OutboundRtpPacket {
    fn from(packet: RtpPacket) -> Self {
        Self {
            payload_type: *packet.header.payload_type,
            sequence_number: packet.header.sequence_number,
            timestamp: packet.header.timestamp.into(),
            marker: packet.header.marker,
            payload: packet.payload,
        }
    }
}

impl From<&RtpPacket> for OutboundRtpPacket {
    fn from(packet: &RtpPacket) -> Self {
        Self {
            payload_type: *packet.header.payload_type,
            sequence_number: packet.header.sequence_number,
            timestamp: packet.header.timestamp.into(),
            marker: packet.header.marker,
            payload: packet.payload.clone(),
        }
    }
}
