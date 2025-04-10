use crate::config::{Committees, EpochNumber, Stake};
use crate::consensus::Round;
use crate::error::{ConsensusError, ConsensusResult};
use crate::messages::{Timeout, Vote, QC, TC};
use crypto::Hash as _;
use crypto::{Digest, PublicKey, Signature};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
#[path = "tests/aggregator_tests.rs"]
pub mod aggregator_tests;

pub struct Aggregator {
    committees: Committees,
    votes_aggregators: HashMap<(EpochNumber, Round), HashMap<Digest, Box<QCMaker>>>,
    timeouts_aggregators: HashMap<(EpochNumber, Round), Box<TCMaker>>,
}

impl Aggregator {
    pub fn new(committees: Committees) -> Self {
        Self {
            committees,
            votes_aggregators: HashMap::new(),
            timeouts_aggregators: HashMap::new(),
        }
    }

    pub fn add_vote(&mut self, vote: Vote) -> ConsensusResult<Option<QC>> {
        // TODO [issue #7]: A bad node may make us run out of memory by sending many votes
        // with different round numbers or different digests.

        // Add the new vote to our aggregator and see if we have a QC.
        self.votes_aggregators
            .entry((vote.epoch, vote.round))
            .or_insert_with(HashMap::new)
            .entry(vote.digest())
            .or_insert_with(|| Box::new(QCMaker::new()))
            .append(vote, &self.committees)
    }

    pub fn add_timeout(&mut self, timeout: Timeout) -> ConsensusResult<Option<TC>> {
        // TODO: A bad node may make us run out of memory by sending many timeouts
        // with different round numbers.

        // Add the new timeout to our aggregator and see if we have a TC.
        self.timeouts_aggregators
            .entry((timeout.epoch, timeout.round))
            .or_insert_with(|| Box::new(TCMaker::new()))
            .append(timeout, &self.committees)
    }

    pub fn cleanup(&mut self, epoch: &EpochNumber, round: &Round) {
        self.votes_aggregators
            .retain(|(vote_epoch, vote_round), _| vote_epoch == epoch && vote_round >= round);
        self.timeouts_aggregators
            .retain(|(timeout_epoch, timeout_round), _| {
                timeout_epoch == epoch && timeout_round >= round
            });
    }
}

struct QCMaker {
    weight: Stake,
    votes: Vec<(PublicKey, Signature)>,
    used: HashSet<PublicKey>,
}

impl QCMaker {
    pub fn new() -> Self {
        Self {
            weight: 0,
            votes: Vec::new(),
            used: HashSet::new(),
        }
    }

    /// Try to append a signature to a (partial) quorum.
    pub fn append(&mut self, vote: Vote, committees: &Committees) -> ConsensusResult<Option<QC>> {
        let author = vote.author;

        // Ensure it is the first time this authority votes.
        ensure!(
            self.used.insert(author),
            ConsensusError::AuthorityReuse(author)
        );

        let vote_committee = committees
            .get_committee_for_epoch(&vote.epoch)
            .ok_or(ConsensusError::UnknownCommittee(vote.epoch))?;

        self.votes.push((author, vote.signature));
        self.weight += vote_committee.stake(&author);
        if self.weight >= vote_committee.quorum_threshold() {
            self.weight = 0; // Ensures QC is only made once.
            return Ok(Some(QC {
                hash: vote.hash.clone(),
                round: vote.round,
                epoch: vote.epoch,
                epoch_concluded: vote.epoch_concluded,
                votes: self.votes.clone(),
            }));
        }
        Ok(None)
    }
}

struct TCMaker {
    weight: Stake,
    votes: Vec<(PublicKey, Signature, EpochNumber, Round)>,
    used: HashSet<PublicKey>,
}

impl TCMaker {
    pub fn new() -> Self {
        Self {
            weight: 0,
            votes: Vec::new(),
            used: HashSet::new(),
        }
    }

    /// Try to append a signature to a (partial) quorum.
    pub fn append(
        &mut self,
        timeout: Timeout,
        committees: &Committees,
    ) -> ConsensusResult<Option<TC>> {
        let author = timeout.author;

        // Ensure it is the first time this authority votes.
        ensure!(
            self.used.insert(author),
            ConsensusError::AuthorityReuse(author)
        );

        let timeout_committee = committees
            .get_committee_for_epoch(&timeout.epoch)
            .ok_or(ConsensusError::UnknownCommittee(timeout.epoch))?;

        // Add the timeout to the accumulator.
        self.votes.push((
            author,
            timeout.signature,
            timeout.high_qc.epoch,
            timeout.high_qc.round,
        ));
        self.weight += timeout_committee.stake(&author);
        if self.weight >= timeout_committee.quorum_threshold() {
            self.weight = 0; // Ensures TC is only created once.
            return Ok(Some(TC {
                epoch: timeout.epoch,
                epoch_concluded: timeout.epoch_concluded,
                round: timeout.round,
                votes: self.votes.clone(),
            }));
        }
        Ok(None)
    }
}
