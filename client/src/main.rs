use anyhow::Error;
use rtc::{Client, OutboundRtpPacket, WebRtcEvent};
use std::net::IpAddr;
use sysinfo::Networks;
use tokio::{
    net::UdpSocket,
    sync::mpsc::{self, Receiver, Sender},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    // * Channel for RTP packets
    let (tx, rx): (Sender<OutboundRtpPacket>, Receiver<OutboundRtpPacket>) = mpsc::channel(100);
    let token = CancellationToken::new();
    let mut set = JoinSet::new();

    set.spawn(media::stream_test_video(tx));
    set.spawn(publish(rx));

    let mut failure: Option<Error> = None;
    while let Some(result) = set.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                token.cancel();
                failure.get_or_insert(e);
                break;
            }
            Err(join_error) => {
                token.cancel();
                failure.get_or_insert(join_error.into());
                break;
            }
        }
    }

    if let Some(err) = failure {
        while set.join_next().await.is_some() {}
        return Err(err);
    }

    Ok(())
}

/// Listens for RTP packets and sends them to the SFU
async fn publish(mut rx: Receiver<OutboundRtpPacket>) -> Result<(), Error> {
    let mut client = Client::new().await.expect("Failed to create client");

    // * Spin up a UDP socket
    let host_addr = select_host_address();
    let socket = match UdpSocket::bind(format!("{host_addr}:0")).await {
        Ok(socket) => socket,
        Err(_) => panic!("failed to bind UDP socket"),
    };
    let socket_addr = match socket.local_addr() {
        Ok(addr) => addr,
        Err(_) => panic!("failed to get session socket address"),
    };
    client.add_local_candidate(socket_addr);

    // TODO: handle error(s)
    client.make_whip_request().await?;

    loop {
        if !client.rtc.is_alive() {
            break;
        }

        // TODO: some reworking here based on the bitwhip repo
        // - The state driving loop (poll output, read & handle input) is part of the client struct. It returns a event (WebRtcEvent) telling the consumer whether it's good to continue, or stop.
        // - If good to continue, it awaits a channel message to write a packet to the stream and then continues the loop
        match client.recv(&socket).await {
            Ok(event) => match event {
                WebRtcEvent::Disconnected => {
                    info!("client disconnected");
                    break;
                }
                WebRtcEvent::RtpPacket(_) => {
                    panic!("publisher received an RTP packet");
                }
                WebRtcEvent::Continue => match rx.try_recv() {
                    Ok(packet) => {
                        client.write_rtp_packet(packet);
                    }
                    Err(e) => match e {
                        mpsc::error::TryRecvError::Empty => {}
                        _ => error!("Unhandled media channel error: {:?}", e),
                    },
                },
            },
            Err(e) => {
                error!("Error from recv(): {:?}", e);
            }
        }
    }
    Ok(())
}

// TODO: this is duplicated from the session actor. Should this be refactored?
// If we move the UDP socket binding logic to the rtc crate, then we need to figure out how
// to get the SocketAddr for calling add_local_candidate()
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
