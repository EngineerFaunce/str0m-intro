use anyhow::Error;
use anyhow::Result;
use reqwest::header::{ACCEPT, HeaderValue};
use reqwest::{ClientBuilder, header::CONTENT_TYPE};
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use str0m::Event;
use str0m::IceConnectionState;
use str0m::Input;
use str0m::Output;
use str0m::media::Mid;
use str0m::rtp::ExtensionValues;
use str0m::rtp::RtpPacket;
use str0m::{
    Rtc, RtcError,
    change::{SdpAnswer, SdpOffer},
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
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
            .build();

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

    pub async fn poll_output(&mut self, socket: &UdpSocket) -> Propagated {
        if !self.rtc.is_alive() {
            return Propagated::Noop;
        }

        match self.rtc.poll_output() {
            Ok(output) => self.handle_output(socket, output).await,
            Err(e) => {
                tracing::warn!("Client ({}) poll_output failed: {:?}", &self.id, e);
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
                    tracing::warn!(
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
                    tracing::trace!("ICE connected and established DTLS.");
                    Propagated::Noop
                }
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    tracing::trace!("ICE disconnected");
                    self.rtc.disconnect();
                    Propagated::Noop
                }
                Event::MediaAdded(media) => {
                    tracing::trace!("Media added: {:?}", media);
                    tracing::trace!("Codec config: {:?}", self.rtc.codec_config());
                    Propagated::Noop
                }
                Event::RtpPacket(packet) => {
                    tracing::trace!("RTP packet: {:?}", packet);
                    Propagated::RtpPacket(self.id, packet)
                }
                _ => Propagated::Noop,
            },
        }
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
                tracing::trace!(
                    "Sent RTP packet: seq={:?}, ts={}",
                    packet.sequence_number,
                    packet.timestamp
                );
            }
            // TODO: handle specific PacketError cases
            Err(e) => {
                tracing::error!("Failed to send RTP packet: {:?}", e);
            }
        }
    }
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
