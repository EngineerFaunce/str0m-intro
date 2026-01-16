use async_channel::{Receiver, Sender, bounded};
use rtc::Client;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub mod session_manager;

/// A video session.
pub struct Session {
    pub id: Uuid,
    publisher: Client,
    // TODO: create a limit on number of subscribers?
    subscribers: HashMap<Uuid, Client>,
    new_subscribers_rx: Receiver<Client>,
}

impl Session {
    /// Creates a session and a channel to send subscribers through.
    pub fn new(publisher: Client) -> (Self, Sender<Client>) {
        // * Channel for receiving WHEP clients later on
        let (tx, rx) = bounded(10);

        (
            Self {
                id: Uuid::new_v4(),
                publisher,
                subscribers: HashMap::new(),
                new_subscribers_rx: rx,
            },
            tx,
        )
    }

    /// Drive the state of the session.
    pub async fn start(&mut self) {
        self.refresh();

        // TODO: Poll the publisher for any RTP packets, or a timeout

        // TODO: call method to forward media from publisher to subscribers.

        // for (id, client) in self.subscribers.iter_mut() {
        //     // TODO: Poll the subscribers until timeout
        // }
        todo!("Do the thing");
    }

    // TODO: better name?
    fn refresh(&mut self) {
        // TODO: check for new subscribers

        self.subscribers.retain(|id, client| {
            if !client.rtc.is_alive() {
                tracing::trace!("Pruning subscriber: {id}");
                false
            } else {
                true
            }
        });
    }
}
