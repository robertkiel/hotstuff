use crate::{epoch_number_from_bytes, Committees};
use bytes::Bytes;
use crypto::{Digest, PublicKey};
use log::{error, warn};
use network::SimpleSender;
use store::{Store, EPOCH_KEY};
use tokio::sync::mpsc::Receiver;

#[cfg(test)]
#[path = "tests/helper_tests.rs"]
pub mod helper_tests;

/// A task dedicated to help other authorities by replying to their batch requests.
pub struct Helper {
    /// The committee information.
    committees: Committees,
    /// The persistent storage.
    store: Store,
    /// Input channel to receive batch requests.
    rx_request: Receiver<(Vec<Digest>, PublicKey)>,
    /// A network sender to send the batches to the other mempools.
    network: SimpleSender,
}

impl Helper {
    pub fn spawn(
        committees: Committees,
        store: Store,
        rx_request: Receiver<(Vec<Digest>, PublicKey)>,
    ) {
        tokio::spawn(async move {
            Self {
                committees,
                store,
                rx_request,
                network: SimpleSender::new(),
            }
            .run()
            .await;
        });
    }

    async fn run(&mut self) {
        while let Some((digests, origin)) = self.rx_request.recv().await {
            // TODO [issue #7]: Do some accounting to prevent bad nodes from monopolizing our resources.

            let epoch = epoch_number_from_bytes(
                self.store
                    .read(EPOCH_KEY.into())
                    .await
                    .expect("Failed to read from store")
                    .expect("Missing epoch entry")
                    .as_slice(),
            );

            // get the requestors address.
            let address = match self
                .committees
                .get_committee_for_epoch(&epoch)
                .expect("No committee for epoch {epoch}")
                .mempool_address(&origin)
            {
                Some(x) => x,
                None => {
                    warn!("Received batch request from unknown authority: {}", origin);
                    continue;
                }
            };

            // Reply to the request (the best we can).
            for digest in digests {
                match self.store.read(digest.to_vec()).await {
                    Ok(Some(data)) => self.network.send(address, Bytes::from(data)).await,
                    Ok(None) => (),
                    Err(e) => error!("{}", e),
                }
            }
        }
    }
}
