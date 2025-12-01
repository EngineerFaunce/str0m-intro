use anyhow::Error;
use anyhow::Result;
use anyhow::anyhow;
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
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::rtp_state::RtpState;

mod rtp_state;

#[derive(Debug)]
pub struct Client {
    pub id: Uuid,
    pub rtc: Rtc,
    pub socket: UdpSocket,
    video_mid: Option<Mid>,
    video_rtp: RtpState,
    buf: [u8; 1500],
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

    pub async fn accept_request(&mut self, offer: SdpOffer) -> Result<String, RtcError> {
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        Ok(answer.to_sdp_string())
    }

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
        // ! Is WHEP different?
        let mut headers = reqwest::header::HeaderMap::new();
        // let header_value = HeaderValue::from_str("application/sdp").unwrap();
        // headers.append(CONTENT_TYPE, header_value.clone());
        // headers.append(ACCEPT, header_value);

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

    /// 
    pub async fn run(&mut self, token: CancellationToken) -> Result<(), Error> {
        loop {
            // * Poll output until we get a timeout. Timeout means we are either awaiting UDP socket input or the timeout to happen.
            let timeout = match self.rtc.poll_output().unwrap() {
                // * Stop polling when we get a timeout
                Output::Timeout(timeout) => timeout,

                // * Transmit this data to the remote peer
                Output::Transmit(send) => {
                    if let Err(e) = self.socket.send_to(&send.contents, send.destination).await {
                        tracing::warn!(
                            "sending to {} => {}, len {} error {:?}",
                            send.source,
                            send.destination,
                            send.contents.len(),
                            e
                        );
                    };
                    continue;
                }
                Output::Event(event) => match event {
                    Event::Connected => {
                        tracing::trace!("ICE connected and established DTLS.");
                        break;
                    }
                    Event::IceConnectionStateChange(state) => {
                        match state {
                            IceConnectionState::Disconnected => {
                                tracing::trace!("ICE disconnected");
                                // TODO: should we error/break here?
                                continue;
                            },
                            IceConnectionState::New => {
                                tracing::trace!("ICE new");
                                continue;
                            },
                            IceConnectionState::Checking => {
                                tracing::trace!("ICE checking");
                                continue;
                            },
                            IceConnectionState::Connected => {
                                tracing::trace!("ICE connected");
                                continue;
                            },
                            IceConnectionState::Completed => {
                                tracing::trace!("ICE complete");
                                continue;
                            },
                        }
                    }
                    Event::MediaAdded(media) => {
                        tracing::trace!("Media added: {:?}", media);
                        tracing::trace!("Codec config: {:?}", self.rtc.codec_config());
                        continue;
                    }
                    Event::MediaData(data) => {
                        tracing::trace!("Media data: {:?}", data);
                        continue;
                    }
                    Event::RtpPacket(packet) => {
                        tracing::trace!("RTP packet: {:?}", packet);
                        continue;
                    }
                    _ => {
                        continue;
                    }
                },
            };

            // * Duration until timeout
            let duration = timeout - Instant::now();

            // * If the duration is zero, drive time forward in rtc straight away
            if duration.is_zero() {
                match self.rtc.handle_input(Input::Timeout(Instant::now())) {
                    Ok(_) => continue,
                    Err(e) => {
                        panic!("error handling input when duration is zero: {:?}", e);
                    }
                };
            }

            // * "Create" the input for the RTC state. This is either by recieving from the UDP socket or by timing out.
            let input = tokio::select! {
                _ = token.cancelled() => break,
                res = tokio::time::timeout(duration, self.socket.recv_from(&mut self.buf)) => {
                    match res {
                        Ok(Ok((n, source))) => {
                            self.buf[n..].fill(0);
                            Input::Receive(Instant::now(), Receive {
                                proto: Protocol::Udp,
                                source,
                                destination: self.socket.local_addr()?,
                                contents: (&self.buf[..n]).try_into()?
                            })
                        }
                        Ok(Err(e)) => match e.kind() {
                            ErrorKind::TimedOut => Input::Timeout(Instant::now()),
                            _ => {
                                tracing::error!("error: {:?}", e);
                                return Err(anyhow!("error receiving from UDP socket: {:?}", e));
                            }
                        },
                        Err(_) => Input::Timeout(Instant::now()),
                    }
                }
            };
            
            // * Drive the state forward with the input.
            self.rtc.handle_input(input).unwrap();
        }

        Ok(())
    }

    pub fn send_video(&mut self, rtp_video_channel: &mut Receiver<Vec<u8>>) -> Result<(), RtcError> {
        // * When there is a video RTP packet to send
        if let Ok(packet) = rtp_video_channel.try_recv() {

            // * Get the parameters for the payload type that 
            let payload_params = self
                .rtc
                .codec_config()
                .find(|p| p.spec().codec == Codec::H264);

            if let Some(params) = payload_params {
                let pt = params.pt();

                // * Get the next sequence number and timestamp
                let (current_seq, ts) = self.video_rtp.next();

                // * Acquire a send stream and write the RTP packet
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
