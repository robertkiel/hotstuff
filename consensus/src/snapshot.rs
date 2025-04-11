use crypto::Digest;
use ed25519_dalek::Digest as _;
use ed25519_dalek::Sha512;
use std::convert::TryInto;

pub fn update_snapshot(current: &Digest, txs: &Vec<Digest>) -> Digest {
    let mut hasher = Sha512::new();

    hasher.update(current);
    for tx in txs {
        hasher.update(tx);
    }

    Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
}
