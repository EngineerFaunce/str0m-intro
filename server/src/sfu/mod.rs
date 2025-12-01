use anyhow::{Error, anyhow};
use rtc::Client;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub enum SessionKind {
    Whip,
    Whep,
}

pub struct SessionClient {
    pub client: Client,
    pub kind: SessionKind,
}

#[derive(Default)]
struct SessionRegistry {
    publisher: Option<Client>,
    // TODO: create a limit on number of subscribers?
    subscribers: HashMap<Uuid, Client>,
}

impl SessionRegistry {
    pub fn add(&mut self, session_client: SessionClient) {
        match session_client.kind {
            SessionKind::Whip => {
                if self.publisher.is_some() {
                    tracing::warn!(
                        "Attempted to assign publisher when one already exists: {}",
                        session_client.client.id
                    );
                }
                self.publisher = Some(session_client.client);
            }
            SessionKind::Whep => {
                self.subscribers
                    .insert(session_client.client.id, session_client.client);
            }
        }
    }

    pub fn prune(&mut self) {
        let drop_publisher = self
            .publisher
            .as_ref()
            .map(|client| !client.rtc.is_alive())
            .unwrap_or(false);

        if drop_publisher {
            if let Some(client) = self.publisher.as_ref() {
                tracing::trace!("Pruning publisher: {}", client.id);
            }
            self.publisher = None;
        }

        // TODO: should likely remove subscribers if there is no publisher

        self.subscribers.retain(|id, client| {
            if !client.rtc.is_alive() {
                tracing::trace!("Pruning subscriber: {id}");
                false
            } else {
                true
            }
        });
    }

    pub async fn drive_clients(&mut self, token: CancellationToken) {
        if let Some(publisher) = self.publisher.as_mut() {
            if let Err(e) = publisher.run(token.clone()).await {
                tracing::debug!("Publisher encountered error: {:?}", e);
            }
        }

        for (id, client) in self.subscribers.iter_mut() {
            if let Err(e) = client.run(token.clone()).await {
                tracing::debug!("Subscriber {id} encountered error: {:?}", e);
            }
        }
    }

    // TODO: implement receiving media from publisher and forwarding to subscribers

}

pub async fn process_clients(
    mut client_channel: Receiver<SessionClient>,
    token: CancellationToken,
) -> Result<(), Error> {
    let mut sessions = SessionRegistry::default();
    // TODO: Is this needed, or is it hindering performance?
    let mut interval = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            _ = token.cancelled() => {
                tracing::debug!("Received cancellation request, shutting down SFU...");
                return Ok(());
            }

            // * Try and receive a new client
            client = client_channel.recv() => {
                match client {
                    Some(session_client) => {
                        tracing::info!("New client: {:?} ({:?})", session_client.client.id, session_client.kind);
                        sessions.add(session_client);
                    }
                    None => {
                        tracing::trace!("Client channel closed, shutting down client processor...");
                        // ! We error here because the server should always be "listening" for new clients if it is running.
                        return Err(anyhow!("client channel closed."));
                    }
                }
            }
            // * On each tick, prune dead clients and drive the state of remaining clients
            _ = interval.tick() => {
                sessions.prune();
                sessions.drive_clients(token.clone()).await;
                // TODO: call method to forward media from publisher to subscribers.
                // ? Is this the actual right place, or should this be a sibling task that is constantly looping? 
                // ? It would if we were going to somehow limit the time we spend forwarding media.
                // ? Should likely review structured concurrency principles and a typical SFU architecture.
            }
        }
    }
}
