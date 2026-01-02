use anyhow::{Error, anyhow};
use rtc::Client;
use std::collections::HashMap;
use tokio::{
    sync::mpsc::{self, Receiver, UnboundedReceiver, UnboundedSender},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

// TODO: move this to a new module?
pub struct Session {
    publisher: Client,
    // TODO: create a limit on number of subscribers?
    subscribers: HashMap<Uuid, Client>,
    new_subscribers_rx: UnboundedReceiver<Client>,
}

impl Session {
    pub fn new(publisher: Client) -> (Self, UnboundedSender<Client>) {
        // * Channel for receiving WHEP clients later on
        let (tx, rx) = mpsc::unbounded_channel();

        (
            Self {
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

pub async fn process_sessions(
    mut session_rx: Receiver<Session>,
    token: CancellationToken,
) -> Result<(), Error> {
    // TODO: rework this logic to handle multiple sessions.
    // I'm thinking that this "main" SFU loop will simply await token cancellation
    // and spawn off new tasks for each session. The sessions will need a channel
    // in order to "send" subscribers to it to start processing.
    // Remember structured concurrency.
    // let mut join_set = JoinSet::new();

    loop {
        tokio::select! {
            // * Try and receive a new session
            session_option = session_rx.recv() => {
                match session_option {
                    Some(session) => {
                        // TODO: spawn a session here
                        // join_set.spawn();
                    }
                    None => {
                        // ! We error here because the server should always be "listening" for new clients if it is running.
                        return Err(anyhow!("client channel closed."));
                    }
                }
            }
            _ = token.cancelled() => {
                tracing::debug!("Received cancellation request, shutting down SFU...");
                return Ok(());
            }
        }
    }
}
