use super::*;
use crate::common::{batch_digest, committee_with_base_port, keys, listener, serialized_batch};
use std::fs;
use tokio::sync::mpsc::channel;

#[tokio::test]
async fn batch_reply() {
    let (tx_request, rx_request) = channel(1);
    let (requestor, _) = keys().pop().unwrap();
    let committees = committee_with_base_port(8_000);

    // Create a new test store.
    let path = ".db_test_batch_reply";
    let _ = fs::remove_dir_all(path);
    let mut store = Store::new(path).unwrap();

    store
        .write(EPOCH_KEY.into(), (1u128.to_be_bytes()).into())
        .await;

    // Add a batch to the store.
    store
        .write(batch_digest().to_vec(), serialized_batch())
        .await;

    // Spawn an `Helper` instance.
    Helper::spawn(committees.clone(), store, rx_request);

    let initial_committee = committees
        .get_committee_for_epoch(&1)
        .expect("Missing committee");

    // Spawn a listener to receive the batch reply.
    let address = initial_committee.mempool_address(&requestor).unwrap();
    let expected = Bytes::from(serialized_batch());
    let handle = listener(address, Some(expected));

    // Send a batch request.
    let digests = vec![batch_digest()];
    tx_request.send((digests, requestor)).await.unwrap();

    // Ensure the requestor received the batch (ie. it did not panic).
    assert!(handle.await.is_ok());
}
