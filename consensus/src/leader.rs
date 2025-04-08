use crate::config::Committees;
use crate::consensus::Round;
use crate::EpochNumber;
use crypto::PublicKey;

pub type LeaderElector = RRLeaderElector;

pub struct RRLeaderElector {
    committees: Committees,
}

impl RRLeaderElector {
    pub fn new(committees: Committees) -> Self {
        Self { committees }
    }

    pub fn get_leader(&self, epoch: EpochNumber, round: Round) -> PublicKey {
        let committee = self.committees.get_committe_for_epoch(&epoch).unwrap();
        let mut keys: Vec<_> = committee.authorities.keys().cloned().collect();
        keys.sort();
        keys[round as usize % committee.size()]
    }
}
