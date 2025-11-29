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
    subscribers: HashMap<Uuid, Client>,
}

impl SessionRegistry {
    pub fn add(&mut self, session_client: SessionClient) {
        match session_client.kind {
            SessionKind::Whip => {
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
}

pub async fn process_clients(
    mut client_channel: Receiver<SessionClient>,
    token: CancellationToken,
) -> Result<(), std::io::Error> {
    let mut sessions = SessionRegistry::default();
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
                        tracing::trace!("New client: {:?} ({:?})", session_client.client.id, session_client.kind);
                        sessions.add(session_client);
                    }
                    None => {
                        tracing::debug!("Client channel closed, shutting down client processor...");
                        return Ok(());
                    }
                }
            }
            _ = interval.tick() => {
                sessions.prune();
                sessions.drive_clients(token.clone()).await;
            }
        }
    }
}
