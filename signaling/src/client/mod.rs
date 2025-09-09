use anyhow::Error;
use anyhow::Result;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks};
use reqwest::header::{HeaderValue, ACCEPT};
use reqwest::{header::CONTENT_TYPE, ClientBuilder};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};
use str0m::format::Codec;
use str0m::media::Mid;
use str0m::net::Protocol;
use str0m::net::Receive;
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
use tracing::debug;
use tracing::info;
use uuid::Uuid;

#[derive(Debug)]
pub struct Client {
    pub id: Uuid,
    pub rtc: Rtc,
    pub socket: UdpSocket,
    video_mid: Option<Mid>,
    buf: [u8; 1500],
}

impl Client {
    pub fn new() -> Result<Self, RtcError> {
        // * Set up the WebRTC client
        let mut rtc = Rtc::builder()
            .set_rtp_mode(true)
            .clear_codecs()
            .enable_h264(true)
            .set_stats_interval(Some(Duration::from_secs(2)))
            .build();

        // TODO: for local testing only - both client and server on same machine
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let socket = UdpSocket::bind(socket_addr).expect("Should bind udp socket");

        let actual_addr = socket.local_addr().expect("Failed to get local addr");
        debug!("local socket address: {:?}", actual_addr);

        rtc.add_local_candidate(
            Candidate::host(actual_addr, str0m::net::Protocol::Udp)
                .expect("Failed to create local candidate"),
        );

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            rtc,
            socket,
            video_mid: None,
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
        headers.append(
            CONTENT_TYPE,
            HeaderValue::from_str("application/sdp").unwrap(),
        );
        headers.append(ACCEPT, HeaderValue::from_str("application/sdp").unwrap());

        let mut buf = Vec::new();

        // TODO: should the certificate and key be moved to a more central location?
        let temp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("self_signed_certs")
            .join("cert.pem");
        let mut file = File::open(temp).await?;
        let bytes_read = file.read_to_end(&mut buf).await?;
        debug!("Read {:?} bytes from cert file.", bytes_read);
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
    pub fn run(&mut self) -> Result<(), Error> {
        let timeout = match self.rtc.poll_output().unwrap() {
            Output::Timeout(timeout) => timeout,
            Output::Transmit(send) => {
                if let Err(e) = self.socket.send_to(&send.contents, send.destination) {
                    debug!(
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
                    info!("connected");
                    return Ok(());
                }
                Event::IceConnectionStateChange(state) => {
                    info!("ice connection state change: {:?}", state);
                    match state {
                        IceConnectionState::Disconnected => {
                            return Err(anyhow::anyhow!("ICE Disconnected"));
                        }
                        _ => return Ok(()),
                    }
                }
                Event::MediaAdded(media) => {
                    info!("Media added: {:?}", media);
                    info!("Codec config: {:?}", self.rtc.codec_config());
                    return Ok(());
                }
                Event::MediaData(data) => {
                    debug!("Media data: {:?}", data);
                    return Ok(());
                }
                Event::RtpPacket(packet) => {
                    debug!("RTP packet: {:?}", packet);
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
            // TODO: error handling
            self.rtc
                .handle_input(Input::Timeout(Instant::now()))
                .unwrap();
            return Ok(());
        }

        self.socket.set_read_timeout(Some(duration)).unwrap();

        let input = match self.socket.recv_from(&mut self.buf) {
            Ok((n, source)) => {
                // UDP data received.
                self.buf[n..].fill(0); // zero out the rest of the buffer
                Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: self.socket.local_addr().unwrap(),
                        contents: self.buf.as_slice().try_into().unwrap(),
                    },
                )
            }
            Err(e) => match e.kind() {
                // Expected error for set_read_timeout().
                // One for windows, one for the rest.
                ErrorKind::WouldBlock | ErrorKind::TimedOut => Input::Timeout(Instant::now()),

                e => {
                    eprintln!("Error: {:?}", e);
                    return Err(anyhow::anyhow!("Socket recv error: {:?}", e));
                }
            },
        };

        // Input is either a Timeout or Receive of data. Both drive the state forward.
        self.rtc.handle_input(input).unwrap();

        Ok(())
    }

    pub fn stream_test_video(&mut self) -> Result<()> {
        gst::init()?;

        // * Set up GStreamer pipeline
        let pipeline = gst::Pipeline::default();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("is-live", true)
            .property_from_str("pattern", "ball")
            .build()?;
        let conv = gst::ElementFactory::make("videoconvert").build()?;
        let enc = gst::ElementFactory::make("x264enc")
            .property_from_str("tune", "zerolatency")
            .build()?;
        let pay = gst::ElementFactory::make("rtph264pay").build()?;
        let sink = gst::ElementFactory::make("appsink")
            .property("emit-signals", true)
            .property("sync", false)
            .build()?;

        pipeline.add_many([&src, &conv, &enc, &pay, &sink])?;
        gst::Element::link_many([&src, &conv, &enc, &pay, &sink])?;

        // * Channel for RTP packets
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();

        // * Set up appsink to capture RTP packets
        let appsink = sink.clone().dynamic_cast::<AppSink>().unwrap();
        appsink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    let data = map.as_slice();
                    // debug!("Got RTP packet: {} bytes", data.len());

                    if let Err(_) = tx.send(data.to_vec()) {
                        return Err(gst::FlowError::Eos);
                    }

                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );

        // TODO: control the pipeline state externally
        pipeline.set_state(gst::State::Playing)?;
        debug!("GStreamer pipeline started");

        // RTP packaet parameters
        let seq_no = Arc::new(AtomicU16::new(1));
        let timestamp = Arc::new(AtomicU32::new(0));
        let start_time = Instant::now();

        let bus = pipeline.bus().unwrap();

        loop {
            if let Some(msg) = bus.pop() {
                match msg.view() {
                    gst::MessageView::Eos(..) => {
                        debug!("End of stream");
                        break;
                    }
                    gst::MessageView::Error(err) => {
                        eprintln!(
                            "Pipeline error from {:?}: {}",
                            err.src().map(|s| s.path_string()),
                            err.error()
                        );
                        break;
                    }
                    _ => {
                        debug!("Other message: {:?}", msg);
                    }
                }
            }

            if let Ok(packet) = rx.try_recv() {
                let payload_params = self
                    .rtc
                    .codec_config()
                    .find(|p| p.spec().codec == Codec::H264);
                if let Some(params) = payload_params {
                    let pt = params.pt();

                    let current_seq = seq_no.fetch_add(1, Ordering::Relaxed);

                    // Calculate timestamp (90kHz clock for video)
                    let elapsed = start_time.elapsed();
                    let ts = (elapsed.as_millis() * 90) as u32;
                    timestamp.store(ts, Ordering::Relaxed);

                    let mut direct_api = self.rtc.direct_api();
                    let stream_tx = direct_api
                        .stream_tx_by_mid(self.video_mid.unwrap(), None)
                        .unwrap();
                    match stream_tx.write_rtp(
                        pt,
                        SeqNo::from(current_seq as u64),
                        ts,
                        Instant::now(),
                        false, // not a marker
                        ExtensionValues::default(),
                        false, // not padding
                        packet,
                    ) {
                        Ok(_) => {
                            debug!("Sent RTP packet: seq={}, ts={}", current_seq, ts);
                        }
                        // TODO: handle specific PacketError cases
                        Err(e) => {
                            debug!("Failed to send RTP packet: {:?}", e);
                            break;
                        }
                    }
                } else {
                    debug!("No payload type found");
                    break;
                }
            }
            // Small sleep to prevent busy waiting
            std::thread::sleep(Duration::from_millis(1));
        }

        pipeline.set_state(gst::State::Null)?;
        debug!("GStreamer pipeline stopped");

        Ok(())
    }
}
