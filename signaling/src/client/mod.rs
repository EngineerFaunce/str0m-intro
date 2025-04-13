use anyhow::Error;
use anyhow::{anyhow, Result};
use gstreamer::{self as gst, prelude::*};
use reqwest::header::{HeaderValue, ACCEPT};
use reqwest::{header::CONTENT_TYPE, ClientBuilder};
use std::path::PathBuf;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};
use str0m::{
    change::{SdpAnswer, SdpOffer},
    Candidate, Rtc, RtcError,
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tracing::{debug, info};
use uuid::Uuid;

#[derive(Debug)]
pub struct Client {
    pub id: Uuid,
    pub rtc: Rtc,
    socket: UdpSocket,
}

impl Client {
    pub async fn make_whip_request(&mut self) -> Result<(), Error> {
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
            rtc,
            socket,
        })
    }

    pub fn stream_test_video(destination: SocketAddr) -> Result<()> {
        gst::init()?;

        let pipeline = gst::Pipeline::default();
        let src = gst::ElementFactory::make("videotestsrc").build()?;
        let conv = gst::ElementFactory::make("videoconvert").build()?;
        let enc = gst::ElementFactory::make("x264enc")
            .property_from_str("tune", "zerolatency")
            .build()?;
        let pay = gst::ElementFactory::make("rtph264pay").build()?;
        let sink = gst::ElementFactory::make("udpsink")
            .property("host", destination.ip().to_string())
            .property("port", destination.port() as i32)
            .build()?;

        pipeline.add_many([&src, &conv, &enc, &pay, &sink])?;
        gst::Element::link_many([&src, &conv, &enc, &pay, &sink])?;

        let bus = pipeline.bus().unwrap();

        pipeline.set_state(gst::State::Playing)?;

        let pipeline_res = bus
            .iter_timed(None)
            .inspect(|msg| {
                if let gst::MessageView::StateChanged(state) = msg.view() {
                    if let Some(element) = msg.src() {
                        if element == &pipeline && state.current() == gst::State::Playing {
                            eprintln!("playing test video");
                            pipeline
                                .debug_to_dot_file(gst::DebugGraphDetails::all(), "server-playing");
                        }
                    }
                }
            })
            .filter_map(|msg| match msg.view() {
                gst::MessageView::Eos(..) => Some(Ok(())),
                gst::MessageView::Error(err) => Some(Err(anyhow!("{err:?}"))),
                _ => None,
            })
            .next()
            .unwrap_or(Err(anyhow!("empty stream")));

        pipeline.set_state(gst::State::Null)?;

        pipeline_res
    }
}
