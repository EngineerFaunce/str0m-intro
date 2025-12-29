use anyhow::{Error, anyhow};
use rtc::Client;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Session {
    publisher: Client,
    // TODO: create a limit on number of subscribers?
    subscribers: HashMap<Uuid, Client>,
}

impl Session {
    pub fn new(publisher: Client) -> Self {
        Self {
            publisher,
            subscribers: HashMap::new(),
        }
    }

    // pub fn add(&mut self, session_client: SessionClient) {
    //     match session_client.kind {
    //         SessionKind::Whip => {
    //             if self.publisher.is_some() {
    //                 tracing::warn!(
    //                     "Attempted to assign publisher when one already exists: {}",
    //                     session_client.client.id
    //                 );
    //             }
    //             self.publisher = Some(session_client.client);
    //         }
    //         SessionKind::Whep => {
    //             self.subscribers
    //                 .insert(session_client.client.id, session_client.client);
    //         }
    //     }
    // }

    pub fn prune(&mut self) {
        // let drop_publisher = self
        //     .publisher
        //     // .as_ref()
        //     .map(|client| !client.rtc.is_alive())
        //     .unwrap_or(false);

        // if drop_publisher {
        //     if let Some(client) = self.publisher.as_ref() {
        //         tracing::trace!("Pruning publisher: {}", client.id);
        //     }
        //     self.publisher = None;
        // }

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
    pub async fn drive_state(&mut self, token: CancellationToken) {
        // TODO: Poll the publisher for any RTP packets, or a timeout
        // if let Some(publisher) = self.publisher.as_mut() {
        // }

        // TODO: call method to forward media from publisher to subscribers.

        for (id, client) in self.subscribers.iter_mut() {
            // TODO: Poll the subscribers until timeout
        }
    }
}

pub async fn process_clients(
    mut client_channel: Receiver<SessionClient>,
    token: CancellationToken,
) -> Result<(), Error> {
    // TODO: rework this logic to handle multiple sessions.
    // I'm thinking that this "main" SFU loop will simply await token cancellation
    // and spawn off new tasks for each session. The sessions will need a channel
    // in order to "send" subscribers to it to start processing.
    // Remember structured concurrency.
    loop {
        tokio::select! {
            // TODO:
            // // * Try and receive a new client
            // client = client_channel.recv() => {
            //     match client {
            //         Some(session_client) => {
            //             tracing::info!("New client: {:?} ({:?})", session_client.client.id, session_client.kind);
            //             // TODO: spawn a session here
            //         }
            //         None => {
            //             tracing::trace!("Client channel closed, shutting down client processor...");
            //             // ! We error here because the server should always be "listening" for new clients if it is running.
            //             return Err(anyhow!("client channel closed."));
            //         }
            //     }
            // }
            _ = token.cancelled() => {
                tracing::debug!("Received cancellation request, shutting down SFU...");
                return Ok(());
            }
        }
    }
}
