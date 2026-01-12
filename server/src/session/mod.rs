use async_channel::{Receiver, Sender, bounded};
use rtc::Client;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

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
    pub async fn start(&mut self) {
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
    use rtc::Client;
    use std::collections::HashMap;
    use tokio::sync::oneshot;
    use uuid::Uuid;

    use crate::{
        session::Session,
        sfu::{SfuHandle, SfuMessage},
    };

    /// Message types to be sent to/from the session tracker
    pub enum SessionMessage {
        EstablishConnection(SfuHandle),
        NewPublisher(Client),
        // TODO: do we add a boolean flag here to indicate if the Ended message came from the SFU process?
        // This is from thinking about the scenario of handling DELETE requests later on
        Ended(Uuid),
        GetActiveSessions(oneshot::Sender<Vec<Uuid>>),
        ValidateSession(Uuid, oneshot::Sender<bool>),
    }

    /// A handle for our custom actor
    pub type SessionManagerHandle = Sender<SessionMessage>;

    /// Custom actor for tracking session activity
    pub struct SessionManager {
        messages_rx: Receiver<SessionMessage>,
        session_registry: HashMap<Uuid, Sender<Client>>,
        sfu_handle: Option<SfuHandle>,
    }

    impl SessionManager {
        /// Create a new actor instance
        pub fn new() -> (Self, SessionManagerHandle) {
            // Channel allowing other processes to message this actor
            let (tx, rx) = channel::bounded(100);
            (
                Self {
                    messages_rx: rx,
                    session_registry: HashMap::new(),
                    sfu_handle: None,
                },
                tx,
            )
        }

        /// Listen for messages and act on them
        pub async fn run(&mut self) -> Result<()> {
            while let Ok(msg) = self.messages_rx.recv().await {
                self.handle_message(msg).await?;
            }
            Ok(())
        }

        async fn handle_message(&mut self, msg: SessionMessage) -> Result<()> {
            match msg {
                SessionMessage::EstablishConnection(handle) => {
                    self.sfu_handle = Some(handle);
                    Ok(())
                }
                SessionMessage::NewPublisher(client) => {
                    let (session, subscriber_tx) = Session::new(client);
                    self.session_registry
                        .insert(session.id.clone(), subscriber_tx);

                    // * Forward the session
                    match &self.sfu_handle {
                        Some(handle) => {
                            if let Err(_) = handle.send(SfuMessage::NewSession(session)).await {
                                tracing::error!("error forwarding session to SFU process.")
                            }
                        }
                        None => {
                            tracing::error!("SFU handle not configured.");
                        }
                    }
                    Ok(())
                }
                SessionMessage::Ended(session_id) => {
                    if !self.session_registry.contains_key(&session_id) {
                        tracing::warn!(
                            "attempted to remove session that is not active: {}",
                            &session_id
                        )
                    }
                    self.session_registry.remove(&session_id);
                    Ok(())
                }
                SessionMessage::GetActiveSessions(response) => {
                    let session_ids: Vec<Uuid> = self.session_registry.keys().copied().collect();
                    if let Err(_) = response.send(session_ids) {
                        tracing::warn!("failed to send list of active sessions");
                    }
                    Ok(())
                }
                SessionMessage::ValidateSession(session_id, response) => {
                    if let Err(_) = response.send(self.session_registry.contains_key(&session_id)) {
                        tracing::error!("failed to verify that session exists")
                    }
                    Ok(())
                }
            }
        }
    }
}
