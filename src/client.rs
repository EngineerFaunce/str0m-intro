use std::{io::ErrorKind, net::{SocketAddr, SocketAddrV4, UdpSocket}, process, thread, time::{Duration, Instant}};

use str0m::{change::{SdpAnswer, SdpOffer, SdpPendingOffer}, net::{Protocol, Receive}, Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcError};
use tracing::info;
use uuid::Uuid;

#[derive(Debug)]
pub struct Client {
    id: Uuid,
    rtc: Rtc,
    pending: Option<SdpPendingOffer>,
    socket: UdpSocket,
    local_socket_addr: Option<SocketAddr>
}

impl Client {
    pub fn new() -> Result<Self, RtcError> {
        let socket = UdpSocket::bind("0.0.0.0".parse::<SocketAddrV4>().unwrap())
            // .await
            .expect("Should bind udp socket");

        let rtc = Rtc::builder()
            .clear_codecs()
            .enable_h264(true)
            .set_stats_interval(Some(Duration::from_secs(2)))
            .set_reordering_size_video(1)
            .set_reordering_size_audio(1)
            .build();

        info!("local socket address: {:?}", socket.local_addr());

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            rtc,
            pending: None,
            socket,
            local_socket_addr: None
        })
    }

    pub fn add_local_candidate(&mut self, socket_addr: &SocketAddr) {
        self.local_socket_addr = Some(socket_addr.clone());

        self.rtc.add_local_candidate(
            Candidate::host(*socket_addr, str0m::net::Protocol::Udp)
                .expect("Failed to create local candidate")
        );
    }

    pub fn create_offer(&mut self) -> Result<SdpOffer, RtcError>{
        let mut change = self.rtc.sdp_api();
        let _mid = change.add_media(str0m::media::MediaKind::Video, str0m::media::Direction::SendRecv, None, None);
        let (offer, pending) = change.apply().unwrap();

        self.pending = Some(pending);

        Ok(offer)
    }

    pub fn create_answer(&mut self, offer: SdpOffer) -> Result<SdpAnswer, RtcError> {
        let answer = self.rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        Ok(answer)
    }

    pub fn accept_answer(&mut self, answer: SdpAnswer) -> Result<(), RtcError> {
        let _ = self.rtc.sdp_api().accept_answer(self.pending.take().unwrap(), answer);
        Ok(())
    }
}
    