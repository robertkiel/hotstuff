use super::*;
use crate::common::{batch, committee_with_base_port, keys, listener};
use crate::mempool::MempoolMessage;
use bytes::Bytes;
use futures::future::try_join_all;
use network::ReliableSender;
use std::fs;
use store::Store;
use tokio::sync::mpsc::channel;

#[tokio::test]
async fn wait_for_quorum() {
    let (tx_message, rx_message) = channel(1);
    let (tx_batch, mut rx_batch) = channel(1);
    let (myself, _) = keys().pop().unwrap();
    let committees = committee_with_base_port(7_000);

    // Create a new test store.
    let path = ".db_test_batch_timeout";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();

    store
        .write(EPOCH_KEY.into(), (1u128.to_be_bytes()).into())
        .await;

    // Spawn a `QuorumWaiter` instance.
    QuorumWaiter::spawn(committees.clone(), store, myself, rx_message, tx_batch);

    // Make a batch.
    let message = MempoolMessage::Batch(batch());
    let serialized = bincode::serialize(&message).unwrap();
    let expected = Bytes::from(serialized.clone());

    // Spawn enough listeners to acknowledge our batches.
    let mut names = Vec::new();
    let mut addresses = Vec::new();
    let mut listener_handles = Vec::new();
    let initial_committee = committees
        .get_committee_for_epoch(&1)
        .expect("Missing committee");

    for (name, address) in initial_committee.broadcast_addresses(&myself) {
        let handle = listener(address, Some(expected.clone()));
        names.push(name);
        addresses.push(address);
        listener_handles.push(handle);
    }

    // Broadcast the batch through the network.
    let bytes = Bytes::from(serialized.clone());
    let handlers = ReliableSender::new().broadcast(addresses, bytes).await;

    // Forward the batch along with the handlers to the `QuorumWaiter`.
    let message = QuorumWaiterMessage {
        batch: serialized.clone(),
        handlers: names.into_iter().zip(handlers.into_iter()).collect(),
    };
    tx_message.send(message).await.unwrap();

    // Wait for the `QuorumWaiter` to gather enough acknowledgements and output the batch.
    let output = rx_batch.recv().await.unwrap();
    assert_eq!(output, serialized);

    // Ensure the other listeners correctly received the batch.
    assert!(try_join_all(listener_handles).await.is_ok());
}
