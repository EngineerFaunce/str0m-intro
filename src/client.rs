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

    pub fn accept_offer(&mut self, answer: SdpAnswer) -> Result<(), RtcError> {
        let _ = self.rtc.sdp_api().accept_answer(self.pending.take().unwrap(), answer);

        // thread::spawn(|| {
        //     if let Err(e) = self.run() {
        //         eprintln!("Exited: {e:?}");
        //         process::exit(1);
        //     }
        // });

        Ok(())
    }

    pub fn create_answer(&mut self, offer: SdpOffer) -> Result<SdpAnswer, RtcError> {
        let answer = self.rtc
            .sdp_api()
            .accept_offer(offer)
            .expect("offer to be accepted");

        Ok(answer)
    }

    // ? The main loop for the client RTC.
    fn run(&mut self) -> Result<(), RtcError> {
        // Buffer for incoming data.
        let mut buf = Vec::new();

        loop {
            // Poll output until we get a timeout. The timeout means we are either awaiting UDP socket input
            // or the timeout to happen.
            let timeout = match self.rtc.poll_output()? {
                Output::Timeout(v) => v,

                Output::Transmit(v) => {
                    self.socket.send_to(&v.contents, v.destination)?;
                    continue;
                }

                Output::Event(v) => {
                    // Abort if we disconnect.
                    if v == Event::IceConnectionStateChange(IceConnectionState::Disconnected) {
                        return Ok(());
                    }

                    // TODO: handle other events, such as incoming media data.

                    continue;
                }
            };

            let timeout = timeout - Instant::now();

            // socket.set_read_timeout(Some(0)) is not ok
            if timeout.is_zero() {
                // Drive time forwards in rtc straight away.
                self.rtc.handle_input(Input::Timeout(Instant::now()))?;
                continue;
            }

            self.socket.set_read_timeout(Some(timeout))?;

            // Scale up buffer to receive an entire UDP packet.
            buf.resize(2000, 0);

            // Try to receive. Because we have a timeout on the socket,
            // we will either receive a packet, or timeout.
            // This is where having an async loop shines. We can await multiple things to
            // happen such as outgoing media data, the timeout and incoming network traffic.
            // When using async there is no need to set timeout on the socket.
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
        }
    }


}
    