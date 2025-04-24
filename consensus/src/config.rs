use crypto::PublicKey;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;

pub type Stake = u32;
pub type EpochNumber = u128;

pub fn epoch_number_from_bytes(bytes: &[u8]) -> EpochNumber {
    let mut epoch = [0u8; (EpochNumber::BITS / 8) as usize];
    epoch.clone_from_slice(bytes);

    EpochNumber::from_be_bytes(epoch)
}

#[derive(Serialize, Deserialize)]
pub struct Parameters {
    pub timeout_delay: u64,
    pub sync_retry_delay: u64,
    pub epoch_len: Option<u64>,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            timeout_delay: 5_000,
            sync_retry_delay: 10_000,
            epoch_len: None,
        }
    }
}

impl Parameters {
    pub fn log(&self) {
        // NOTE: These log entries are used to compute performance.
        info!("Timeout delay set to {} rounds", self.timeout_delay);
        info!("Sync retry delay set to {} ms", self.sync_retry_delay);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Authority {
    pub stake: Stake,
    pub address: SocketAddr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Committee {
    pub authorities: HashMap<PublicKey, Authority>,
    pub epoch: EpochNumber,
}

impl Committee {
    pub fn new(info: Vec<(PublicKey, Stake, SocketAddr)>, epoch: EpochNumber) -> Self {
        Self {
            authorities: info
                .into_iter()
                .map(|(name, stake, address)| {
                    let authority = Authority { stake, address };
                    (name, authority)
                })
                .collect(),
            epoch,
        }
    }

    pub fn size(&self) -> usize {
        self.authorities.len()
    }

    pub fn stake(&self, name: &PublicKey) -> Stake {
        self.authorities.get(name).map_or_else(|| 0, |x| x.stake)
    }

    pub fn quorum_threshold(&self) -> Stake {
        // If N = 3f + 1 + k (0 <= k < 3)
        // then (2 N + 3) / 3 = 2f + 1 + (2k + 2)/3 = 2f + 1 + k = N - f
        let total_votes: Stake = self.authorities.values().map(|x| x.stake).sum();
        2 * total_votes / 3 + 1
    }

    pub fn address(&self, name: &PublicKey) -> Option<SocketAddr> {
        self.authorities.get(name).map(|x| x.address)
    }

    pub fn broadcast_addresses(&self, myself: &PublicKey) -> Vec<(PublicKey, SocketAddr)> {
        self.authorities
            .iter()
            .filter(|(name, _)| name != &myself)
            .map(|(name, x)| (*name, x.address))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Committees {
    pub committees: HashMap<EpochNumber, Committee>,
}

impl Committees {
    pub fn new() -> Self {
        Self {
            committees: Default::default(),
        }
    }

    pub fn add_committe_for_epoch(&mut self, committee: Committee, epoch: EpochNumber) {
        if self.committees.insert(epoch, committee).is_some() {
            warn!("Replacing existing consensus committee for epoch {epoch}")
        }
    }

    pub fn get_committee_for_epoch(&self, epoch: &EpochNumber) -> Option<Committee> {
        // TODO: think of a generic way to assign committees for epochs
        self.committees.get(&epoch).map(|c| c.to_owned())
    }
}
