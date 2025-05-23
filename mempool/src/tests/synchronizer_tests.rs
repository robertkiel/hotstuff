use super::*;
use crate::common::{batch_digest, committee_with_base_port, keys, listener};
use std::fs;
use tokio::sync::mpsc::channel;

#[tokio::test]
async fn synchronize() {
    let (tx_message, rx_message) = channel(1);

    let mut keys = keys();
    let (name, _) = keys.pop().unwrap();
    let committee = committee_with_base_port(9_000);

    // Create a new test store.
    let path = ".db_test_synchronize";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();

    store
        .write(EPOCH_KEY.into(), (1u128.to_be_bytes()).into())
        .await;

    // Spawn a `Synchronizer` instance.
    Synchronizer::spawn(
        name,
        committee.clone(),
        store.clone(),
        /* gc_depth */ 50, // Not used in this test.
        /* sync_retry_delay */ 1_000_000, // Ensure it is not triggered.
        /* sync_retry_nodes */ 3, // Not used in this test.
        rx_message,
    );

    let initial_committee = committee
        .get_committee_for_epoch(&1)
        .expect("Missing committee for epoch {epoch}");

    // Spawn a listener to receive our batch requests.
    let (target, _) = keys.pop().unwrap();
    let address = initial_committee.mempool_address(&target).unwrap();
    let missing = vec![batch_digest()];
    let message = MempoolMessage::BatchRequest(missing.clone(), name);
    let serialized = bincode::serialize(&message).unwrap();
    let handle = listener(address, Some(Bytes::from(serialized)));

    // Send a sync request.
    let message = ConsensusMempoolMessage::Synchronize(missing, target);
    tx_message.send(message).await.unwrap();

    // Ensure the target receives the sync request.
    assert!(handle.await.is_ok());
}
