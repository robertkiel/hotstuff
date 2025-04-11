use crate::config::{Committees, EpochNumber};
use crate::consensus::Round;
use crate::error::{ConsensusError, ConsensusResult};
use crate::snapshot::update_snapshot;
use crypto::{Digest, Hash, PublicKey, Signature, SignatureService};
use ed25519_dalek::Digest as _;
use ed25519_dalek::Sha512;
use log::error;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::convert::TryInto;
use std::fmt;

#[cfg(test)]
#[path = "tests/messages_tests.rs"]
pub mod messages_tests;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Block {
    pub qc: QC,
    pub tc: Option<TC>,
    pub author: PublicKey,
    pub round: Round,
    pub payload: Vec<Digest>,
    pub signature: Signature,
    pub epoch: EpochNumber,
    pub epoch_concluded: bool,
    pub snapshot: Digest,
}

impl Block {
    pub async fn new(
        qc: QC,
        tc: Option<TC>,
        author: PublicKey,
        round: Round,
        payload: Vec<Digest>,
        epoch: EpochNumber,
        epoch_concluded: bool,
        snapshot: Digest,
        mut signature_service: SignatureService,
    ) -> Self {
        let block = Self {
            qc,
            tc,
            author,
            round,
            snapshot: update_snapshot(&snapshot, &payload),
            payload,
            epoch,
            epoch_concluded,
            signature: Signature::default(),
        };
        let signature = signature_service.request_signature(block.digest()).await;
        Self { signature, ..block }
    }

    pub fn genesis() -> Self {
        Block::default()
    }

    pub fn parent(&self) -> &Digest {
        &self.qc.hash
    }

    pub fn verify(&self, committees: &Committees) -> ConsensusResult<()> {
        let block_committee = committees
            .get_committee_for_epoch(&self.epoch)
            .ok_or(ConsensusError::UnknownCommittee(self.epoch))?;

        // Ensure the authority has voting rights.
        let voting_rights = block_committee.stake(&self.author);
        ensure!(
            voting_rights > 0,
            ConsensusError::UnknownAuthority(self.author)
        );

        // Check the signature.
        self.signature.verify(&self.digest(), &self.author)?;

        // Check the embedded QC.
        if self.qc != QC::genesis() {
            self.qc.verify(committees)?;
        }

        ensure!(
            block_committee.epoch == self.epoch,
            ConsensusError::InvalidEpoch(self.epoch, block_committee.epoch)
        );

        // Check the TC embedded in the block (if any).
        if let Some(ref tc) = self.tc {
            tc.verify(committees)?;
        }
        Ok(())
    }
}

impl Hash for Block {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(self.author.0);
        hasher.update(self.round.to_le_bytes());
        hasher.update(self.epoch.to_le_bytes());
        if self.epoch_concluded {
            hasher.update([1u8]);
        } else {
            hasher.update([0u8]);
        }
        for x in &self.payload {
            hasher.update(x);
        }
        hasher.update(&self.qc.hash);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}: B({}, epoch {} round {}, {:?}, round_concluded {}, {})",
            self.digest(),
            self.author,
            self.epoch,
            self.round,
            self.qc,
            self.epoch_concluded,
            self.payload.iter().map(|x| x.size()).sum::<usize>(),
        )
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "B{}", self.round)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Vote {
    pub hash: Digest,
    pub round: Round,
    pub epoch: EpochNumber,
    pub epoch_concluded: bool,
    pub last_snapshot: Digest,
    pub author: PublicKey,
    pub signature: Signature,
}

impl Vote {
    pub async fn new(
        block: &Block,
        author: PublicKey,
        mut signature_service: SignatureService,
    ) -> Self {
        let vote = Self {
            hash: block.digest(),
            epoch: block.epoch,
            epoch_concluded: block.epoch_concluded,
            round: block.round,
            last_snapshot: block.snapshot.to_owned(),
            author,
            signature: Signature::default(),
        };
        let signature = signature_service.request_signature(vote.digest()).await;
        Self { signature, ..vote }
    }

    pub fn verify(&self, committees: &Committees) -> ConsensusResult<()> {
        let vote_committee = committees
            .get_committee_for_epoch(&self.epoch)
            .ok_or(ConsensusError::UnknownCommittee(self.epoch))?;
        // Ensure the authority has voting rights.
        ensure!(
            vote_committee.stake(&self.author) > 0,
            ConsensusError::UnknownAuthority(self.author)
        );

        // Check the signature.
        self.signature
            .verify(&self.digest(), &self.author)
            .inspect_err(|_| error!("Failed to verify signature for Vote {self:?}"))?;
        Ok(())
    }
}

impl Hash for Vote {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(&self.hash);
        hasher.update(self.round.to_le_bytes());
        hasher.update(self.epoch.to_le_bytes());
        if self.epoch_concluded {
            hasher.update([1u8]);
        } else {
            hasher.update([0u8]);
        }
        hasher.update(&self.last_snapshot);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for Vote {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "V({}, {}, {})", self.author, self.round, self.hash)
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct QC {
    pub hash: Digest,
    pub epoch: EpochNumber,
    pub epoch_concluded: bool,
    pub last_snapshot: Digest,
    pub round: Round,
    pub votes: Vec<(PublicKey, Signature)>,
}

impl QC {
    pub fn genesis() -> Self {
        QC::default()
    }

    pub fn timeout(&self) -> bool {
        self.hash == Digest::default() && self.round != 0
    }

    pub fn verify(&self, committees: &Committees) -> ConsensusResult<()> {
        let qc_committee = committees
            .get_committee_for_epoch(&self.epoch)
            .ok_or(ConsensusError::UnknownCommittee(self.epoch))?;
        // Ensure the QC has a quorum.
        let mut weight = 0;
        let mut used = HashSet::new();
        for (name, _) in self.votes.iter() {
            ensure!(!used.contains(name), ConsensusError::AuthorityReuse(*name));
            let voting_rights = qc_committee.stake(name);
            ensure!(voting_rights > 0, ConsensusError::UnknownAuthority(*name));
            used.insert(*name);
            weight += voting_rights;
        }
        ensure!(
            weight >= qc_committee.quorum_threshold(),
            ConsensusError::QCRequiresQuorum
        );

        // Check the signatures.
        Signature::verify_batch(&self.digest(), &self.votes).map_err(ConsensusError::from)
    }
}

impl Hash for QC {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(&self.hash);
        hasher.update(self.round.to_le_bytes());
        hasher.update(self.epoch.to_le_bytes());
        if self.epoch_concluded {
            hasher.update([1u8]);
        } else {
            hasher.update([0u8])
        }
        hasher.update(&self.last_snapshot);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for QC {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "QC({}, {})", self.hash, self.round)
    }
}

impl PartialEq for QC {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.round == other.round
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Timeout {
    pub high_qc: QC,
    pub epoch: EpochNumber,
    pub epoch_concluded: bool,
    pub last_snapshot: Digest,
    pub round: Round,
    pub author: PublicKey,
    pub signature: Signature,
}

impl Timeout {
    pub async fn new(
        high_qc: QC,
        epoch: EpochNumber,
        epoch_concluded: bool,
        last_snapshot: Digest,
        round: Round,
        author: PublicKey,
        mut signature_service: SignatureService,
    ) -> Self {
        let timeout = Self {
            high_qc,
            round,
            epoch,
            epoch_concluded,
            last_snapshot,
            author,
            signature: Signature::default(),
        };
        let signature = signature_service.request_signature(timeout.digest()).await;
        Self {
            signature,
            ..timeout
        }
    }

    pub fn verify(&self, committees: &Committees) -> ConsensusResult<()> {
        let timeout_committee = committees
            .get_committee_for_epoch(&self.epoch)
            .ok_or(ConsensusError::UnknownCommittee(self.epoch))?;

        // Ensure the authority has voting rights.
        ensure!(
            timeout_committee.stake(&self.author) > 0,
            ConsensusError::UnknownAuthority(self.author)
        );

        // Check the signature.
        self.signature
            .verify(&self.digest(), &self.author)
            .inspect_err(|_| error!("Failed to verify signatuere for Timeout {self:?}"))?;

        // Check the embedded QC.
        if self.high_qc != QC::genesis() {
            self.high_qc.verify(committees)?;
        }
        Ok(())
    }
}

impl Hash for Timeout {
    fn digest(&self) -> Digest {
        let mut hasher = Sha512::new();
        hasher.update(self.round.to_le_bytes());
        hasher.update(self.epoch.to_le_bytes());
        if self.epoch_concluded {
            hasher.update([1u8]);
        } else {
            hasher.update([0u8]);
        }
        hasher.update(self.high_qc.round.to_le_bytes());
        hasher.update(self.high_qc.epoch.to_le_bytes());
        hasher.update(&self.last_snapshot);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Debug for Timeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "TV({}, {}, {:?})", self.author, self.round, self.high_qc)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TC {
    pub epoch: EpochNumber,
    pub epoch_concluded: bool,
    pub last_snapshot: Digest,
    pub round: Round,
    pub votes: Vec<(PublicKey, Signature, EpochNumber, Round)>,
}

impl TC {
    pub fn verify(&self, committees: &Committees) -> ConsensusResult<()> {
        let tc_committee = committees
            .get_committee_for_epoch(&self.epoch)
            .ok_or(ConsensusError::UnknownCommittee(self.epoch))?;
        // Ensure the QC has a quorum.
        let mut weight = 0;
        let mut used = HashSet::new();
        for (name, _, _, _) in self.votes.iter() {
            ensure!(!used.contains(name), ConsensusError::AuthorityReuse(*name));
            let voting_rights = tc_committee.stake(name);
            ensure!(voting_rights > 0, ConsensusError::UnknownAuthority(*name));
            used.insert(*name);
            weight += voting_rights;
        }
        ensure!(
            weight >= tc_committee.quorum_threshold(),
            ConsensusError::TCRequiresQuorum
        );

        // Check the signatures.
        for (author, signature, high_qc_epoch, high_qc_round) in &self.votes {
            let mut hasher = Sha512::new();
            hasher.update(self.round.to_le_bytes());
            hasher.update(self.epoch.to_le_bytes());
            if self.epoch_concluded {
                hasher.update([1u8]);
            } else {
                hasher.update([0u8]);
            }
            hasher.update(high_qc_round.to_le_bytes());
            hasher.update(high_qc_epoch.to_le_bytes());
            hasher.update(&self.last_snapshot);
            let digest = Digest(hasher.finalize().as_slice()[..32].try_into().unwrap());
            signature.verify(&digest, author).inspect_err(|_| {
                error!("Failed to verify signature for TimeoutCertificate {self:?}")
            })?;
        }
        Ok(())
    }

    pub fn high_qc_rounds(&self) -> Vec<(EpochNumber, Round)> {
        self.votes.iter().map(|(_, _, e, r)| (*e, *r)).collect()
    }
}

impl fmt::Debug for TC {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "TC(epoch {} round {}, {:?})",
            self.epoch,
            self.round,
            self.high_qc_rounds()
        )
    }
}
