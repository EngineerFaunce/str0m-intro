use async_channel::{Receiver, Sender, TryRecvError, bounded};
use rtc::{Client, Propagated};
use std::{
    collections::{HashMap, VecDeque},
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    time::Instant,
};
use str0m::{
    Candidate, Input,
    net::{Protocol, Receive},
};
use sysinfo::Networks;
use tokio::net::UdpSocket;
use uuid::Uuid;

pub mod session_manager;

/// A video session.
pub struct Session {
    pub id: Uuid,
    publisher: Client,
    socket: UdpSocket,
    socket_addr: SocketAddr,
    // TODO: create a limit on number of subscribers?
    subscribers: HashMap<Uuid, Client>,
    new_subscribers_rx: Receiver<Client>,
}

impl Session {
    /// Creates a session and a channel to send subscribers through.
    pub async fn new(mut publisher: Client) -> (Self, Sender<Client>) {
        // * Spin up a UDP socket that we'll multiplex/demultiplex over
        let host_addr = select_host_address();
        let socket = match UdpSocket::bind(format!("{host_addr}:0")).await {
            Ok(socket) => socket,
            Err(_) => panic!("failed to bind UDP socket"),
        };
        let socket_addr = match socket.local_addr() {
            Ok(addr) => addr,
            Err(_) => panic!("failed to get session socket address"),
        };

        publisher.rtc.add_local_candidate(
            Candidate::host(socket_addr, str0m::net::Protocol::Udp).expect("a host candidate"),
        );

        // * Channel for receiving WHEP clients later on
        let (tx, rx) = bounded(10);

        (
            Self {
                id: Uuid::new_v4(),
                publisher,
                socket,
                socket_addr,
                subscribers: HashMap::new(),
                new_subscribers_rx: rx,
            },
            tx,
        )
    }

    /// Drive the state of the session.
    pub async fn start(&mut self) {
        let mut to_propagate: VecDeque<Propagated> = VecDeque::new();
        let mut buf = vec![0; 2000];
        loop {
            self.refresh();

            let _t = self.poll_until_timeout(&mut to_propagate).await;

            // TODO: call method to forward media from publisher to subscribers.
            // If we have an item to propagate, do that
            if let Some(p) = to_propagate.pop_front() {
                self.propagate(&p);
                continue;
            }

            // ? No need to set socket read timeout since we're using tokio::net::UdpSocket?

            for (id, client) in self.subscribers.iter_mut() {
                tracing::trace!("polling subscriber: {:?}", id);
                let _propagated = client.poll_output(&self.socket).await;
                // TODO: do we need to do anything with the output, or do we only care about driving the state forward?
            }

            if let Some(input) = read_socket_input(&self.socket, &mut buf).await {
                // The rtc.accepts() call is how we demultiplex the incoming packet to know which Rtc instance the traffic belongs to
                if self.publisher.accepts(&input) {
                    self.publisher.handle_input(input);
                } else if let Some((_, client)) =
                    self.subscribers.iter_mut().find(|(_, c)| c.accepts(&input))
                {
                    // We found the client that accepts the input.
                    client.handle_input(input);
                } else {
                    // This is quite common because we don't get the Rtc instance via the mpsc channel
                    // quickly enough before the browser send the first STUN.
                    tracing::debug!("No client accepts UDP input: {:?}", input);
                }
            }
        }
    }

    // TODO: better name?
    /// Refreshes the session state by pruning disconnected clients and checking for new subscribers
    fn refresh(&mut self) {
        self.subscribers.retain(|id, client| {
            if !client.rtc.is_alive() {
                tracing::trace!("Pruning subscriber: {id}");
                false
            } else {
                true
            }
        });

        match self.new_subscribers_rx.try_recv() {
            Ok(mut subscriber) => {
                subscriber.rtc.add_local_candidate(
                    Candidate::host(self.socket_addr, str0m::net::Protocol::Udp)
                        .expect("a host candidate"),
                );

                // TODO: do something with option here?
                let _res = self.subscribers.insert(subscriber.id, subscriber);
            }
            Err(TryRecvError::Empty) => tracing::trace!("subscriber channel empty"),
            _ => panic!("Subscriber receiver disconnected."),
        }
    }

    async fn poll_until_timeout(&mut self, queue: &mut VecDeque<Propagated>) -> Instant {
        loop {
            if !self.publisher.rtc.is_alive() {
                return Instant::now();
            }

            let propagated = self.publisher.poll_output(&self.socket).await;

            if let Propagated::Timeout(t) = propagated {
                return t;
            }

            queue.push_back(propagated)
        }
    }

    fn propagate(&mut self, packet: &Propagated) {
        if let Propagated::RtpPacket(id, p) = packet {
            tracing::trace!("RTP packet from publisher {}", id);

            for (_, client) in self.subscribers.iter_mut() {
                client.write_rtp_packet(p);
            }
        }
    }
}

async fn read_socket_input<'a>(socket: &UdpSocket, buf: &'a mut Vec<u8>) -> Option<Input<'a>> {
    buf.resize(2000, 0);

    match socket.recv_from(buf).await {
        Ok((n, source)) => {
            buf.truncate(n);

            // Parse data to a DatagramRecv
            let Ok(contents) = buf.as_slice().try_into() else {
                return None;
            };

            Some(Input::Receive(
                Instant::now(),
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination: socket.local_addr().unwrap(),
                    contents,
                },
            ))
        }

        Err(e) => match e.kind() {
            // Expected error for set_read_timeout(). One for windows, one for the rest.
            ErrorKind::WouldBlock | ErrorKind::TimedOut => None,
            _ => panic!("UdpSocket read failed: {e:?}"),
        },
    }
}

/// Returns an available IP address on the host machine
fn select_host_address() -> IpAddr {
    let networks = Networks::new_with_refreshed_list();
    for (_interface_name, network) in &networks {
        for ip_network in network.ip_networks() {
            if let IpAddr::V4(v) = ip_network.addr {
                if !v.is_loopback() && !v.is_link_local() && !v.is_broadcast() {
                    return IpAddr::V4(v);
                }
            }
        }
    }
    panic!("Found no usable network interface");
}
