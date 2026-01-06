use anyhow::{Error, anyhow};
use async_channel::Receiver;
use tokio_util::sync::CancellationToken;

use crate::session::tracking::Handle;

pub async fn process_sessions(
    mut session_manager: Handle,
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
            // TODO: receive messages and start sessions
            _ = token.cancelled() => {
                tracing::debug!("Received cancellation request, shutting down SFU...");
                return Ok(());
            }
        }
    }
}
