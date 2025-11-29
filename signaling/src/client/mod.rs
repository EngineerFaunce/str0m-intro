use anyhow::Error;
use anyhow::Result;
use rand::Rng;
use reqwest::header::{HeaderValue, ACCEPT};
use reqwest::{header::CONTENT_TYPE, ClientBuilder};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::Instant;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use str0m::format::Codec;
use str0m::media::Mid;
use str0m::net::{Protocol, Receive};
use str0m::rtp::ExtensionValues;
use str0m::rtp::SeqNo;
use str0m::Event;
use str0m::IceConnectionState;
use str0m::Input;
use str0m::Output;
use str0m::{
    change::{SdpAnswer, SdpOffer},
    Candidate, Rtc, RtcError,
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Receiver;
use uuid::Uuid;

#[derive(Debug)]
pub struct Client {
    pub id: Uuid,
    pub rtc: Rtc,
    pub socket: UdpSocket,
    video_mid: Option<Mid>,
    video_rtp: RtpState,
    buf: [u8; 1500],
}

#[derive(Debug)]
struct RtpState {
    seq_no: u16,
    ts_base: u32,
    start_time: Instant,
}

impl RtpState {
    fn new() -> Self {
        let mut rng = rand::thread_rng();
        Self {
            seq_no: rng.gen::<u16>(),
            ts_base: rng.gen::<u32>(),
            start_time: Instant::now(),
        }
    }

    fn next(&mut self) -> (SeqNo, u32) {
        let seq = self.seq_no;
        self.seq_no = self.seq_no.wrapping_add(1);

        // 90kHz RTP clock for H.264 video
        let elapsed_90khz = (self.start_time.elapsed().as_micros() * 90) as u32;
        let ts = self.ts_base.wrapping_add(elapsed_90khz);

        (SeqNo::from(seq as u64), ts)
    }
}

impl Client {
    pub async fn new() -> Result<Self, RtcError> {
        // * Set up the WebRTC client
        let mut rtc = Rtc::builder()
            .set_rtp_mode(true)
            .clear_codecs()
            .enable_h264(true)
            .set_stats_interval(Some(Duration::from_secs(2)))
            .build();

        // TODO: for local testing only - both client and server on same machine
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let socket = UdpSocket::bind(socket_addr).await?;

        let actual_addr = socket.local_addr()?;
        tracing::debug!("local socket address: {:?}", actual_addr);

        rtc.add_local_candidate(Candidate::host(actual_addr, str0m::net::Protocol::Udp)?);

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            rtc,
            socket,
            video_mid: None,
            video_rtp: RtpState::new(),
            buf: [0u8; 1500],
        })
    }

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

    pub async fn accept_whip_request(&mut self, offer: SdpOffer) -> Result<String, RtcError> {
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        Ok(answer.to_sdp_string())
    }

    // TODO: refactor to return Result and handle errors in caller
    pub async fn run(&mut self) -> Result<(), Error> {
        let timeout = match self.rtc.poll_output().unwrap() {
            Output::Timeout(timeout) => timeout,
            Output::Transmit(send) => {
                if let Err(e) = self.socket.send_to(&send.contents, send.destination).await {
                    tracing::debug!(
                        "sending to {} => {}, len {} error {:?}",
                        send.source,
                        send.destination,
                        send.contents.len(),
                        e
                    );
                };
                return Ok(());
            }
            Output::Event(event) => match event {
                Event::Connected => {
                    tracing::trace!("connected");
                    return Ok(());
                }
                Event::IceConnectionStateChange(state) => {
                    tracing::trace!("ice connection state change: {:?}", state);
                    match state {
                        IceConnectionState::Disconnected => {
                            return Err(anyhow::anyhow!("ICE Disconnected"));
                        }
                        _ => return Ok(()),
                    }
                }
                Event::MediaAdded(media) => {
                    tracing::trace!("Media added: {:?}", media);
                    tracing::trace!("Codec config: {:?}", self.rtc.codec_config());
                    return Ok(());
                }
                Event::MediaData(data) => {
                    tracing::trace!("Media data: {:?}", data);
                    return Ok(());
                }
                Event::RtpPacket(packet) => {
                    tracing::trace!("RTP packet: {:?}", packet);
                    return Ok(());
                }
                _ => {
                    return Ok(());
                }
            },
        };

        let duration = timeout - Instant::now();
        if duration.is_zero() {
            // Drive time forward in rtc straight away
            return match self.rtc.handle_input(Input::Timeout(Instant::now())) {
                Ok(_) => Ok(()),
                Err(e) => {
                    tracing::error!("error handling input: {:?}", e);
                    Ok(())
                }
            };
        }

        let input = match tokio::time::timeout(duration, self.socket.recv_from(&mut self.buf)).await
        {
            Ok(Ok((n, source))) => {
                // UDP data received.
                tracing::trace!(
                    "received from {} => {}, len {}",
                    source,
                    self.socket.local_addr().unwrap(),
                    n
                );
                self.buf[n..].fill(0); // zero out the rest of the buffer
                Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: self.socket.local_addr().unwrap(),
                        contents: (&self.buf[..n]).try_into().expect("should webrtc"),
                    },
                )
            }
            Ok(Err(e)) => match e.kind() {
                ErrorKind::ConnectionReset => return Ok(()),
                _ => {
                    return Err(anyhow::anyhow!("[TransportWebrtc] network error {:?}", e));
                }
            },
            Err(_e) => Input::Timeout(Instant::now()),
        };

        // Input is either a Timeout or Receive of data. Both drive the state forward.
        self.rtc.handle_input(input).unwrap();

        Ok(())
    }

    pub fn send_video(&mut self, receive_channel: &mut Receiver<Vec<u8>>) -> Result<(), RtcError> {
        if let Ok(packet) = receive_channel.try_recv() {
            let payload_params = self
                .rtc
                .codec_config()
                .find(|p| p.spec().codec == Codec::H264);
            if let Some(params) = payload_params {
                let pt = params.pt();

                let (current_seq, ts) = self.video_rtp.next();

                let mut direct_api = self.rtc.direct_api();
                let stream_tx = direct_api
                    .stream_tx_by_mid(self.video_mid.unwrap(), None)
                    .unwrap();
                match stream_tx.write_rtp(
                    pt,
                    current_seq,
                    ts,
                    Instant::now(),
                    false, // not a marker
                    ExtensionValues::default(),
                    false, // not padding
                    packet,
                ) {
                    Ok(_) => {
                        tracing::trace!("Sent RTP packet: seq={:?}, ts={}", current_seq, ts);
                    }
                    // TODO: handle specific PacketError cases
                    Err(e) => {
                        tracing::error!("Failed to send RTP packet: {:?}", e);
                    }
                }
            } else {
                tracing::debug!("No payload type found");
            }
        }

        Ok(())
    }
}
