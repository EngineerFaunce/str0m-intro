use anyhow::{Error, Result};
use async_channel::{self as channel, Receiver, Sender};
use rtc::Client;
use tokio_util::sync::CancellationToken;

use crate::session::{Session, tracking::SessionManagerHandle};

pub enum SfuMessage {
    EstablishConnection(SessionManagerHandle),
    NewSession(Session),
    NewSubscriber(Client),
}

pub type SfuHandle = Sender<SfuMessage>;

/// Selective Forwarding Unit
pub struct Sfu {
    messages_rx: Receiver<SfuMessage>,
    session_manager_handle: Option<SessionManagerHandle>,
}

impl Sfu {
    pub fn new() -> (Self, SfuHandle) {
        let (tx, rx) = channel::bounded(100);
        (
            Self {
                messages_rx: rx,
                session_manager_handle: None,
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

    async fn handle_message(&mut self, msg: SfuMessage) -> Result<()> {
        match msg {
            SfuMessage::EstablishConnection(handle) => {
                self.session_manager_handle = Some(handle);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub async fn process_sessions(&mut self, token: CancellationToken) -> Result<(), Error> {
        // TODO: rework this logic to handle multiple sessions.
        // I'm thinking that this "main" SFU loop will simply await token cancellation
        // and spawn off new tasks for each session. The sessions will need a channel
        // in order to "send" subscribers to it to start processing.
        // Remember structured concurrency.
        // let mut join_set = JoinSet::new();

        loop {
            tokio::select! {
                // TODO: receive messages and start sessions
                _ = token.cancelled() => {
                    tracing::debug!("Received cancellation request, shutting down SFU...");
                    return Ok(());
                }
            }
        }
    }
}
