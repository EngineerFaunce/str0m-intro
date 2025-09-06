use anyhow::Error;
use anyhow::Result;
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::{AppSink, AppSinkCallbacks};
use reqwest::header::{HeaderValue, ACCEPT};
use reqwest::{header::CONTENT_TYPE, ClientBuilder};
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
use str0m::media::Mid;
use str0m::media::Pt;
use str0m::rtp::ExtensionValues;
use str0m::rtp::SeqNo;
use str0m::{
    change::{SdpAnswer, SdpOffer},
    Candidate, Rtc, RtcError,
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tracing::debug;
use uuid::Uuid;

#[derive(Debug)]
pub struct Client {
    pub id: Uuid,
    pub rtc: Rtc,
    pub socket: UdpSocket,
    video_mid: Option<Mid>,
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

        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let socket = UdpSocket::bind(socket_addr).expect("Should bind udp socket");
        debug!("local socket address: {:?}", socket.local_addr());

        rtc.add_local_candidate(
            Candidate::host(socket_addr, str0m::net::Protocol::Udp)
                .expect("Failed to create local candidate"),
        );

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            rtc,
            socket,
            video_mid: None,
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

    pub fn stream_test_video(&mut self) -> Result<()> {
        gst::init()?;

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

        // Channel for RTP packets
        let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();

        let appsink = sink.clone().dynamic_cast::<AppSink>().unwrap();

        appsink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    let data = map.as_slice();
                    debug!("Got RTP packet: {} bytes", data.len());

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
                let config = self.rtc.codec_config();
                let payload_params = config.find(|_params| true);
                if let Some(params) = payload_params {
                    let pt = params.pt();
                    // debug!("Using payload type: {:?}", pt);

                    let mut direct_api = self.rtc.direct_api();
                    let stream_tx = direct_api
                        .stream_tx_by_mid(self.video_mid.unwrap(), None)
                        .unwrap();

                    let current_seq = seq_no.fetch_add(1, Ordering::Relaxed);

                    // Calculate timestamp (90kHz clock for video)
                    let elapsed = start_time.elapsed();
                    let ts = (elapsed.as_millis() * 90) as u32;
                    timestamp.store(ts, Ordering::Relaxed);

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
            // debug!("Looping...");
            // Small sleep to prevent busy waiting
            std::thread::sleep(Duration::from_millis(1));
        }

        pipeline.set_state(gst::State::Null)?;
        debug!("GStreamer pipeline stopped");

        Ok(())
    }
}
