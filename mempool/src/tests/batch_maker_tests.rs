use super::*;
use crate::common::{committee_with_base_port, keys, transaction};
use std::fs;
use tokio::sync::mpsc::channel;

#[tokio::test]
async fn make_batch() {
    let (tx_transaction, rx_transaction) = channel(1);
    let (tx_message, mut rx_message) = channel(1);
    let (myself, _) = keys().pop().unwrap();

    // Create a new test store.
    let path = ".db_make_batch";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();

    store
        .write(EPOCH_KEY.into(), (1u128.to_be_bytes()).into())
        .await;

    let committees = committee_with_base_port(7_000);

    // Spawn a `BatchMaker` instance.
    BatchMaker::spawn(
        /* max_batch_size */ 200,
        /* max_batch_delay */ 1_000_000, // Ensure the timer is not triggered.
        rx_transaction,
        tx_message,
        committees,
        store,
        myself,
    );

    // Send enough transactions to seal a batch.
    tx_transaction.send(transaction()).await.unwrap();
    tx_transaction.send(transaction()).await.unwrap();

    // Ensure the batch is as expected.
    let expected_batch = vec![transaction(), transaction()];
    let QuorumWaiterMessage { batch, handlers: _ } = rx_message.recv().await.unwrap();
    match bincode::deserialize(&batch).unwrap() {
        MempoolMessage::Batch(batch) => assert_eq!(batch, expected_batch),
        _ => panic!("Unexpected message"),
    }
}

#[tokio::test]
async fn batch_timeout() {
    let (tx_transaction, rx_transaction) = channel(1);
    let (tx_message, mut rx_message) = channel(1);
    let (myself, _) = keys().pop().unwrap();

    // Create a new test store.
    let path = ".db_test_batch_timeout";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();

    store
        .write(EPOCH_KEY.into(), (1u128.to_be_bytes()).into())
        .await;

    let committees = committee_with_base_port(7_000);

    // Spawn a `BatchMaker` instance.
    BatchMaker::spawn(
        /* max_batch_size */ 200,
        /* max_batch_delay */ 50, // Ensure the timer is triggered.
        rx_transaction,
        tx_message,
        committees,
        store,
        myself,
    );

    // Do not send enough transactions to seal a batch..
    tx_transaction.send(transaction()).await.unwrap();

    // Ensure the batch is as expected.
    let expected_batch = vec![transaction()];
    let QuorumWaiterMessage { batch, handlers: _ } = rx_message.recv().await.unwrap();
    match bincode::deserialize(&batch).unwrap() {
        MempoolMessage::Batch(batch) => assert_eq!(batch, expected_batch),
        _ => panic!("Unexpected message"),
    }
}
