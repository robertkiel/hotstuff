use crate::config::Export as _;
use crate::config::{Committees, ConfigError, Parameters, Secret};
use consensus::{Block, Consensus};
use crypto::SignatureService;
use log::info;
use mempool::{epoch_number_from_bytes, Mempool};
use store::{Store, EPOCH_KEY};
use tokio::sync::mpsc::{channel, Receiver};

/// The default channel capacity for this module.
pub const CHANNEL_CAPACITY: usize = 1_000;

pub struct Node {
    pub commit: Receiver<Block>,
}

impl Node {
    pub async fn new(
        committee_file: &str,
        key_file: &str,
        store_path: &str,
        parameters: Option<String>,
    ) -> Result<Self, ConfigError> {
        let (tx_commit, rx_commit) = channel(CHANNEL_CAPACITY);
        let (tx_consensus_to_mempool, rx_consensus_to_mempool) = channel(CHANNEL_CAPACITY);
        let (tx_mempool_to_consensus, rx_mempool_to_consensus) = channel(CHANNEL_CAPACITY);

        // Read the committee and secret key from file.
        let committee = Committees::read(committee_file)?;
        let secret = Secret::read(key_file)?;
        let name = secret.name;
        let secret_key = secret.secret;

        // Load default parameters if none are specified.
        let parameters = match parameters {
            Some(filename) => Parameters::read(&filename)?,
            None => Parameters::default(),
        };

        // Make the data store.
        let mut store = Store::new(store_path).expect("Failed to create store");

        let epoch = match store
            .read(EPOCH_KEY.into())
            .await
            .expect("Failed to read from store")
        {
            Some(raw_epoch) => epoch_number_from_bytes(raw_epoch.as_slice()),
            None => {
                info!("No epoch found in store. Setting epoch to 1");
                store
                    .write(EPOCH_KEY.into(), (1u128.to_be_bytes()).into())
                    .await;
                1
            }
        };

        // Run the signature service.
        let signature_service = SignatureService::new(secret_key);

        // Make a new mempool.
        Mempool::spawn(
            name,
            committee.mempool,
            parameters.mempool,
            epoch,
            store.clone(),
            rx_consensus_to_mempool,
            tx_mempool_to_consensus,
        );

        // Run the consensus core.
        Consensus::spawn(
            name,
            committee.consensus,
            epoch,
            parameters.consensus.epoch_len,
            parameters.consensus,
            signature_service,
            store,
            rx_mempool_to_consensus,
            tx_consensus_to_mempool,
            tx_commit,
        );

        info!("Node {} successfully booted", name);
        Ok(Self { commit: rx_commit })
    }

    pub fn print_key_file(filename: &str) -> Result<(), ConfigError> {
        Secret::new().write(filename)
    }

    pub async fn analyze_block(&mut self) {
        while let Some(_block) = self.commit.recv().await {
            // This is where we can further process committed block.
            //
            // TODO: Implement fast sync:
            // Once a new epoch block has been committed, we aggregate all blocks from the previous epoch and send them to other nodees (if asked).
        }
    }
}
