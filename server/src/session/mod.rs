use async_channel::{Receiver, Sender, TryRecvError, bounded};
use rtc::{Client, Propagated};
use std::{
    collections::{HashMap, VecDeque},
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};
use str0m::{
    Input,
    net::{Protocol, Receive},
};
use sysinfo::Networks;
use tokio::net::UdpSocket;
use tracing::{info, trace, warn};
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

        publisher.add_local_candidate(socket_addr);

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
            if !self.publisher.rtc.is_alive() {
                info!("Publisher disconnected. Ending session.");
                break;
            }
            self.refresh();

            trace!("Polling publisher: {}", self.publisher.id);
            let mut timeout = self
                .publisher
                .poll_until_timeout(&self.socket, Some(&mut to_propagate))
                .await;

            // If we have an item to propagate, do so
            if let Some(p) = to_propagate.pop_front() {
                self.propagate(&p);
                // ? Why are we continuing here? Is it to ensure we drain the queue?
                continue;
            }

            for (id, client) in self.subscribers.iter_mut() {
                trace!("Polling subscriber: {:?}", id);
                timeout = client.poll_until_timeout(&self.socket, None).await;
            }

            let duration = (timeout - Instant::now()).max(Duration::from_millis(20));

            let result =
                tokio::time::timeout(duration, read_socket_input(&self.socket, &mut buf)).await;
            match result {
                Ok(option) => {
                    if let Some(input) = option {
                        info!("Read socket input. Determining client that accepts.");
                        // The rtc.accepts() call is how we demultiplex the incoming packet to know which Rtc instance the traffic belongs to
                        if self.publisher.accepts(&input) {
                            info!("Publisher accepts input.");
                            self.publisher.handle_input(input);
                        } else if let Some((_, client)) =
                            self.subscribers.iter_mut().find(|(_, c)| c.accepts(&input))
                        {
                            client.handle_input(input);
                        } else {
                            // TODO: does this occur and if so, should we handle it somehow?
                            warn!("No client accepts UDP input: {:?}", input);
                        }
                    } else {
                        warn!("No socket input.");
                    }
                }
                Err(_) => {
                    warn!("Reading socket input timed out.");
                }
            }
            // Drive state forward for session
            trace!("Driving session state forward.");
            self.publisher.handle_input(Input::Timeout(Instant::now()));
            for (_, client) in self.subscribers.iter_mut() {
                client.handle_input(Input::Timeout(Instant::now()));
            }
        }
    }

    // TODO: better name?
    /// Refreshes the session state by pruning disconnected clients and checking for new subscribers
    fn refresh(&mut self) {
        if !self.subscribers.is_empty() {
            trace!("Pruning disconnected subscribers from session: {}", self.id);
            self.subscribers.retain(|id, client| {
                if !client.rtc.is_alive() {
                    trace!("Pruning subscriber: {id}");
                    false
                } else {
                    true
                }
            });
        }

        match self.new_subscribers_rx.try_recv() {
            Ok(mut subscriber) => {
                trace!("Adding new subscriber to session: {}", subscriber.id);
                // TODO: is this necessary?
                subscriber.add_local_candidate(self.socket_addr);
                // TODO: do something with option here?
                let _res = self.subscribers.insert(subscriber.id, subscriber);
            }
            Err(TryRecvError::Empty) => {
                // trace!("subscriber channel empty");
            }
            Err(e) => {
                panic!("unhandled subscriber channel error: {:?}", e);
            }
        }
    }

    /// Transmits a RTP packet to every subscriber
    fn propagate(&mut self, packet: &Propagated) {
        if let Propagated::RtpPacket(id, p) = packet {
            trace!("Forwarding RTP packet from publisher: {}", id);

            for (_, client) in self.subscribers.iter_mut() {
                client.write_rtp_packet(p);
            }
        } else {
            trace!("No RTP packet to forward.");
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

            trace!("Received datagram message from {}", source);

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
            ErrorKind::WouldBlock | ErrorKind::TimedOut => Some(Input::Timeout(Instant::now())),
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
