use anyhow::Result;
use async_channel::{self as channel, Receiver, Sender};
use rtc::Client;
use std::collections::HashMap;
use tokio::{sync::oneshot, task::JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::session::Session;

/// Message types to be sent to/from the session tracker
pub enum SessionMessage {
    NewPublisher(Client),
    GetActiveSessions(oneshot::Sender<Vec<Uuid>>),
    ValidateSession(Uuid, oneshot::Sender<bool>),
    // TODO: handle DELETE requests for ending a session
}

/// A handle for our custom actor
pub type SessionManagerHandle = Sender<SessionMessage>;

/// Custom actor for tracking session activity
pub struct SessionManager {
    messages_rx: Receiver<SessionMessage>,
    session_registry: HashMap<Uuid, Sender<Client>>,
    join_set: JoinSet<Uuid>,
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
                join_set: JoinSet::new(),
            },
            tx,
        )
    }

    /// Listen for messages and act on them
    pub async fn run(&mut self, token: CancellationToken) -> Result<()> {
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    tracing::debug!("Received cancellation request, shutting down SFU...");
                    return Ok(());
                }

                Ok(msg) = self.messages_rx.recv() => {
                    self.handle_message(msg).await?;
                }

                Some(Ok(session_id)) = self.join_set.join_next() => {
                    self.session_registry.remove(&session_id);
                }
            }
        }
    }

    async fn handle_message(&mut self, msg: SessionMessage) -> Result<()> {
        match msg {
            SessionMessage::NewPublisher(client) => {
                let (mut session, subscriber_tx) = Session::new(client);
                self.session_registry
                    .insert(session.id.clone(), subscriber_tx);

                self.join_set.spawn(async move {
                    session.start().await;
                    session.id
                });
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
