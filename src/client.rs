use crate::util::get_random_ip_address;
use std::{
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};
use str0m::{
    change::{SdpAnswer, SdpOffer, SdpPendingOffer},
    net::{Protocol, Receive},
    Candidate, Event, Input, Output, Rtc, RtcError,
};
use tracing::info;
use uuid::Uuid;

pub enum WebRtcEvent {
    Continue,
    Disconnected,
}

#[derive(Debug)]
pub struct Client {
    pub id: Uuid,
    rtc: Rtc,
    pending: Option<SdpPendingOffer>,
    socket: UdpSocket,
}

impl Client {
    pub fn new() -> Result<Self, RtcError> {
        let socket_addr = SocketAddr::new(get_random_ip_address(), 0);
        let socket = UdpSocket::bind(socket_addr).expect("Should bind udp socket");

        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_h264(true)
            .set_stats_interval(Some(Duration::from_secs(2)))
            .set_reordering_size_video(1)
            .set_reordering_size_audio(1)
            .build();

        info!("local socket address: {:?}", socket.local_addr());

        let candidate = Candidate::host(socket_addr, str0m::net::Protocol::Udp)
            .expect("Failed to create local candidate");
        rtc.add_local_candidate(candidate);

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            rtc,
            pending: None,
            socket,
        })
    }

    pub fn create_offer(&mut self) -> Result<SdpOffer, RtcError> {
        let mut change = self.rtc.sdp_api();
        let _mid = change.add_media(
            str0m::media::MediaKind::Video,
            str0m::media::Direction::SendRecv,
            None,
            None,
        );
        let (offer, pending) = change.apply().unwrap();

        self.pending = Some(pending);

        Ok(offer)
    }

    pub fn create_answer(&mut self, offer: SdpOffer) -> Result<SdpAnswer, RtcError> {
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        Ok(answer)
    }

    pub fn accept_answer(&mut self, answer: SdpAnswer) -> Result<(), RtcError> {
        let _ = self
            .rtc
            .sdp_api()
            .accept_answer(self.pending.take().unwrap(), answer);
        Ok(())
    }

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
