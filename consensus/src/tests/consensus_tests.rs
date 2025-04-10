use super::*;
use crate::common::{committees_with_base_port, keys};
use crate::config::Parameters;
use crypto::SecretKey;
use futures::future::try_join_all;
use std::fs;
use store::EPOCH_KEY;
use tokio::sync::mpsc::channel;
use tokio::task::JoinHandle;

fn spawn_nodes(
    keys: Vec<(PublicKey, SecretKey)>,
    committees: Committees,
    store_path: &str,
) -> Vec<JoinHandle<Block>> {
    keys.into_iter()
        .enumerate()
        .map(|(i, (name, secret))| {
            let committees = committees.clone();
            let parameters = Parameters {
                timeout_delay: 100,
                ..Parameters::default()
            };
            let store_path = format!("{}_{}", store_path, i);
            let _ = fs::remove_dir_all(&store_path);
            let mut store = Store::new(&store_path).unwrap();

            let signature_service = SignatureService::new(secret);
            let (tx_consensus_to_mempool, mut rx_consensus_to_mempool) = channel(10);
            let (_tx_mempool_to_consensus, rx_mempool_to_consensus) = channel(1);
            let (tx_commit, mut rx_commit) = channel(1);

            // Sink the mempool channel.
            tokio::spawn(async move {
                loop {
                    rx_consensus_to_mempool.recv().await;
                }
            });

            // Spawn the consensus engine.
            tokio::spawn(async move {
                store
                    .write(EPOCH_KEY.into(), (1u128.to_be_bytes()).into())
                    .await;

                Consensus::spawn(
                    name,
                    committees,
                    1,
                    None,
                    parameters,
                    signature_service,
                    store,
                    rx_mempool_to_consensus,
                    tx_consensus_to_mempool,
                    tx_commit,
                );

                rx_commit.recv().await.unwrap()
            })
        })
        .collect()
}

#[tokio::test]
async fn end_to_end() {
    let committees = committees_with_base_port(15_000);

    let store_path = ".db_test_end_to_end";

    // Run all nodes.
    let handles = spawn_nodes(keys(), committees, store_path);

    // Ensure all threads terminated correctly.
    let blocks = try_join_all(handles).await.unwrap();
    assert!(blocks.windows(2).all(|w| w[0] == w[1]));
}
