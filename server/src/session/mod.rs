use rtc::Client;
use std::collections::HashMap;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A video session.
pub struct Session {
    pub id: Uuid,
    publisher: Client,
    // TODO: create a limit on number of subscribers?
    subscribers: HashMap<Uuid, Client>,
    new_subscribers_rx: UnboundedReceiver<Client>,
}

impl Session {
    /// Creates a session and a channel to send subscribers through.
    pub fn new(publisher: Client) -> (Self, UnboundedSender<Client>) {
        // * Channel for receiving WHEP clients later on
        let (tx, rx) = mpsc::unbounded_channel();

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

    // TODO: better name?
    pub fn refresh(&mut self) {
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

    /// Drive the state of the session.
    pub async fn start(&mut self, token: CancellationToken) {
        self.refresh();

        // TODO: Poll the publisher for any RTP packets, or a timeout

        // TODO: call method to forward media from publisher to subscribers.

        // for (id, client) in self.subscribers.iter_mut() {
        //     // TODO: Poll the subscribers until timeout
        // }
        todo!("Do the thing");
    }
}

pub mod tracking {
    use anyhow::Result;
    use async_channel::{self as channel, Receiver, Sender};
    use futures_concurrency::prelude::*;
    use uuid::Uuid;

    /// Message types to be sent to/from the session tracker
    pub enum Message {
        Created(Uuid),
        Ended(Uuid),
    }

    /// A handle for our custom actor
    type Handle = Sender<Message>;

    /// Custom actor for tracking session activity
    pub struct SessionTracker(Receiver<Message>);

    impl SessionTracker {
        /// Create a new actor instance
        pub fn new() -> (Self, Handle) {
            let (sender, receiver) = channel::bounded(100);
            (Self(receiver), sender)
        }

        /// Listen for messages and act on them
        pub async fn run(&mut self) -> Result<()> {
            // self.0
            //     .co()
            //     .try_for_each(|msg| async {
            //         todo!("handle message here");
            //     })
            //     .await;
            Ok(())
        }
    }

    async fn handle_message(msg: Message) -> Result<()> {
        match msg {
            Message::Created(session_id) => {
                todo!("forward session to SFU and notify HTTP server")
            }
            Message::Ended(session_id) => {
                todo!("notify HTTP server that session ended")
            }
        }
        // Ok(())
    }
}
