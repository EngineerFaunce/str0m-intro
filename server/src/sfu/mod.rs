use anyhow::Result;
use async_channel::{self as channel, Receiver, Sender};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::session::{
    Session,
    tracking::{SessionManagerHandle, SessionMessage},
};

pub enum SfuMessage {
    EstablishConnection(SessionManagerHandle),
    NewSession(Session),
}

pub type SfuHandle = Sender<SfuMessage>;

/// Selective Forwarding Unit
pub struct Sfu {
    messages_rx: Receiver<SfuMessage>,
    session_manager_handle: Option<SessionManagerHandle>,
    join_set: JoinSet<Uuid>,
}

impl Sfu {
    pub fn new() -> (Self, SfuHandle) {
        let (tx, rx) = channel::bounded(100);
        (
            Self {
                messages_rx: rx,
                session_manager_handle: None,
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
                    if let Some(handle) = &self.session_manager_handle {
                        handle.send(SessionMessage::Ended(session_id))
                            .await
                            .ok();
                    } else {
                        tracing::error!("session ended but no session manager handle available");
                    }
                }
            }
        }
    }

    async fn handle_message(&mut self, msg: SfuMessage) -> Result<()> {
        match msg {
            SfuMessage::EstablishConnection(handle) => {
                self.session_manager_handle = Some(handle);
                Ok(())
            }
            SfuMessage::NewSession(mut session) => {
                self.join_set.spawn(async move {
                    session.start().await;
                    session.id
                });
                Ok(())
            }
        }
    }
}
