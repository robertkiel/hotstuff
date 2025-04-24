use crate::aggregator::Aggregator;
use crate::config::Committees;
use crate::consensus::{ConsensusMessage, Round};
use crate::error::{ConsensusError, ConsensusResult};
use crate::leader::LeaderElector;
use crate::mempool::MempoolDriver;
use crate::messages::{Block, Timeout, Vote, QC, TC};
use crate::proposer::ProposerMessage;
use crate::snapshot::update_snapshot;
use crate::synchronizer::Synchronizer;
use crate::timer::Timer;
use crate::EpochNumber;
use async_recursion::async_recursion;
use bytes::Bytes;
use crypto::{Digest, Hash as _};
use crypto::{PublicKey, SignatureService};
use log::{debug, error, info, warn};
use network::SimpleSender;
use std::collections::VecDeque;
use store::{Store, EPOCH_KEY};
use tokio::sync::mpsc::{Receiver, Sender};

#[cfg(test)]
#[path = "tests/core_tests.rs"]
pub mod core_tests;

/// Takes a tuple consisting of epoch and round and returns the subsequent tuple.
/// If there is no epoch_len specified, i.e. `epoch_len == None`, the next tuple is just the round incremented by one.
/// Otherwise,
pub fn next(epoch_len: Option<u64>, epoch: EpochNumber, round: Round) -> (EpochNumber, Round) {
    if epoch_len.is_some_and(|epoch_len| round == epoch_len) {
        (epoch + 1, 1)
    } else {
        (epoch, round + 1)
    }
}

/// Takes two tuples, `a` and `b`, consisting round and epoch and returns true if `b` is the direct successor of `a`.
pub fn is_subsequent(
    epoch_len: Option<u64>,
    a_epoch: EpochNumber,
    a_round: Round,
    b_epoch: EpochNumber,
    b_round: Round,
) -> bool {
    if epoch_len.is_some_and(|epoch_len| a_round == epoch_len) {
        a_epoch + 1 == b_epoch && b_round == 1
    } else {
        a_round + 1 == b_round
    }
}

/// Takes two tuples, `a` and `b`, consisting of round and epoch and returns true if `a` is before `b`.
pub fn is_before(
    epoch_len: Option<u64>,
    a_epoch: EpochNumber,
    a_round: Round,
    b_epoch: EpochNumber,
    b_round: Round,
) -> bool {
    match epoch_len {
        Some(epoch_len) => {
            a_epoch < b_epoch
                || a_epoch == b_epoch && a_round < b_round
                || a_round == epoch_len && a_epoch + 1 == b_epoch && b_epoch == 1
        }
        None => a_round < b_round,
    }
}

pub struct Core {
    name: PublicKey,
    committees: Committees,
    epoch: EpochNumber,
    epoch_len: Option<u64>,
    store: Store,
    signature_service: SignatureService,
    leader_elector: LeaderElector,
    mempool_driver: MempoolDriver,
    synchronizer: Synchronizer,
    rx_message: Receiver<ConsensusMessage>,
    rx_loopback: Receiver<Block>,
    tx_proposer: Sender<ProposerMessage>,
    tx_commit: Sender<Block>,
    round: Round,
    last_voted_round: Round,
    last_voted_epoch: EpochNumber,
    last_committed_round: Round,
    last_committed_epoch: EpochNumber,
    last_snapshot: Option<Digest>,
    high_qc: QC,
    timer: Timer,
    aggregator: Aggregator,
    network: SimpleSender,
}

impl Core {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        name: PublicKey,
        committees: Committees,
        epoch: EpochNumber,
        epoch_len: Option<u64>,
        last_snapshot: Option<Digest>,
        signature_service: SignatureService,
        store: Store,
        leader_elector: LeaderElector,
        mempool_driver: MempoolDriver,
        synchronizer: Synchronizer,
        timeout_delay: u64,
        rx_message: Receiver<ConsensusMessage>,
        rx_loopback: Receiver<Block>,
        tx_proposer: Sender<ProposerMessage>,
        tx_commit: Sender<Block>,
    ) {
        tokio::spawn(async move {
            Self {
                name,
                committees: committees.clone(),
                epoch,
                epoch_len,
                signature_service,
                store,
                leader_elector,
                mempool_driver,
                synchronizer,
                rx_message,
                rx_loopback,
                tx_proposer,
                tx_commit,
                last_snapshot,
                round: 1,
                last_voted_round: 0,
                last_voted_epoch: 0,
                last_committed_round: 0,
                last_committed_epoch: 0,
                high_qc: QC::genesis(),
                timer: Timer::new(timeout_delay),
                aggregator: Aggregator::new(committees),
                network: SimpleSender::new(),
            }
            .run()
            .await
        });
    }

    async fn store_block(&mut self, block: &Block) {
        let key = block.digest().to_vec();
        let value = bincode::serialize(block).expect("Failed to serialize block");
        self.store.write(key, value).await;
    }

    async fn update_epoch(&mut self, epoch: &EpochNumber) {
        self.store
            .write(EPOCH_KEY.into(), epoch.to_be_bytes().into())
            .await;
        self.epoch = epoch.to_owned();
    }

    fn increase_last_voted(&mut self, target_epoch: EpochNumber, target_round: Round) {
        if is_before(
            self.epoch_len,
            self.last_voted_epoch,
            self.last_voted_round,
            target_epoch,
            target_round,
        ) {
            self.last_voted_epoch = target_epoch;
            self.last_voted_round = target_round;
        }
    }

    async fn make_vote(&mut self, block: &Block) -> Option<Vote> {
        // Check if we can vote for this block.
        let safety_rule_1 = is_before(
            self.epoch_len,
            self.last_voted_epoch,
            self.last_voted_round,
            block.epoch,
            block.round,
        );
        let mut safety_rule_2 = is_subsequent(
            self.epoch_len,
            block.qc.epoch,
            block.qc.round,
            block.epoch,
            block.round,
        );
        if let Some(ref tc) = block.tc {
            let mut can_extend =
                is_subsequent(self.epoch_len, tc.epoch, tc.round, block.epoch, block.round);

            let mut highest = None;

            for (epoch, round) in tc.high_qc_rounds() {
                match highest {
                    None => {
                        highest = Some((epoch, round));
                    }
                    Some((highest_epoch, highest_round)) => {
                        if is_before(self.epoch_len, highest_epoch, highest_round, epoch, round) {
                            highest = Some((highest_epoch, highest_round));
                        }
                    }
                }
            }

            let highest = highest.expect("Empty TC");
            can_extend &= block.qc.epoch > highest.0
                || block.qc.epoch == highest.0 && block.qc.round >= highest.1;

            safety_rule_2 |= can_extend;
        }
        if !(safety_rule_1 && safety_rule_2) {
            return None;
        }

        // Ensure we won't vote for contradicting blocks.
        self.increase_last_voted(block.epoch, block.round);

        // TODO [issue #15]: Write to storage preferred_round and last_voted_round.
        Some(Vote::new(block, self.name, self.signature_service.clone()).await)
    }

    async fn commit(&mut self, block: Block) -> ConsensusResult<()> {
        if block.epoch_concluded && self.epoch_len.is_none() {
            error!("Tried to commit to epoch change block but epoch changes are not enabled");
            return Err(ConsensusError::InvalidPayload);
        }

        if is_before(
            self.epoch_len,
            block.epoch,
            block.round,
            self.last_committed_epoch,
            self.last_committed_round,
        ) {
            return Ok(());
        }

        // Ensure we commit the entire chain. This is needed after view-change.
        let mut to_commit = VecDeque::new();
        let mut parent = block.clone();

        let next_after_last_committed = next(
            self.epoch_len,
            self.last_committed_epoch,
            self.last_committed_round,
        );

        while is_before(
            self.epoch_len,
            next_after_last_committed.0,
            next_after_last_committed.1,
            parent.epoch,
            parent.round,
        ) {
            let ancestor = self
                .synchronizer
                .get_parent_block(&parent)
                .await?
                .expect("We should have all the ancestors by now");
            to_commit.push_front(ancestor.clone());
            parent = ancestor;
        }
        to_commit.push_front(block.clone());

        // Save the last committed block.
        self.last_committed_round = block.round;
        self.last_committed_epoch = block.epoch;

        // Send all the newly committed blocks to the node's application layer.
        while let Some(block) = to_commit.pop_back() {
            if !block.payload.is_empty() {
                info!("Committed {}", block);

                #[cfg(feature = "benchmark")]
                for x in &block.payload {
                    // NOTE: This log entry is used to compute performance.
                    info!("Committed {} -> {:?}", block, x);
                }
            }
            debug!("Committed {:?}", block);
            if let Err(e) = self.tx_commit.send(block).await {
                warn!("Failed to send block through the commit channel: {}", e);
            }
        }
        Ok(())
    }

    fn update_high_qc(&mut self, qc: &QC) {
        if is_before(
            self.epoch_len,
            self.high_qc.epoch,
            self.high_qc.round,
            qc.epoch,
            qc.round,
        ) {
            self.high_qc = qc.clone();
        }
    }

    async fn local_timeout_round(&mut self) -> ConsensusResult<()> {
        warn!(
            "Timeout reached for epoch {} round {}",
            self.epoch, self.round
        );

        // Increase the last voted round.
        self.increase_last_voted(self.epoch, self.round);

        let epoch_concluded;
        if matches!(self.epoch_len, Some(epoch_len) if self.round == epoch_len) {
            epoch_concluded = true;
        } else {
            epoch_concluded = false;
        }
        // Make a timeout message.
        let timeout = Timeout::new(
            self.high_qc.clone(),
            self.epoch,
            epoch_concluded,
            self.last_snapshot
                .as_ref()
                .map(|s| s.to_owned())
                .unwrap_or_default(),
            self.round,
            self.name,
            self.signature_service.clone(),
        )
        .await;
        debug!("Created {:?}", timeout);

        // Reset the timer.
        self.timer.reset();

        let timeout_committee = self
            .committees
            .get_committee_for_epoch(&self.epoch)
            .ok_or(ConsensusError::UnknownCommittee(self.epoch))?;

        // Broadcast the timeout message.
        debug!("Broadcasting {:?}", timeout);
        let addresses = timeout_committee
            .broadcast_addresses(&self.name)
            .into_iter()
            .map(|(_, x)| x)
            .collect();
        let message = bincode::serialize(&ConsensusMessage::Timeout(timeout.clone()))
            .expect("Failed to serialize timeout message");
        self.network
            .broadcast(addresses, Bytes::from(message))
            .await;

        // Process our message.
        self.handle_timeout(&timeout).await
    }

    #[async_recursion]
    async fn handle_vote(&mut self, vote: &Vote) -> ConsensusResult<()> {
        debug!("Processing Vote {:?}", vote);
        if vote.epoch != self.epoch || vote.round < self.round {
            return Ok(());
        }

        // Ensure the vote is well formed.
        vote.verify(&self.committees)?;

        // Add the new vote to our aggregator and see if we have a quorum.
        if let Some(qc) = self.aggregator.add_vote(vote.clone())? {
            debug!("Assembled {:?}", qc);

            // Process the QC.
            self.process_qc(&qc).await;

            // Make a new block if we are the next leader.
            if self.name == self.leader_elector.get_leader(self.epoch, self.round) {
                self.generate_proposal(None, vote.last_snapshot.to_owned())
                    .await;
            }
        }
        Ok(())
    }

    async fn handle_timeout(&mut self, timeout: &Timeout) -> ConsensusResult<()> {
        debug!("Processing Timeout {:?}", timeout);
        if timeout.epoch != self.epoch || timeout.round < self.round {
            return Ok(());
        }

        // Ensure the timeout is well formed.
        timeout.verify(&self.committees)?;

        // Process the QC embedded in the timeout.
        self.process_qc(&timeout.high_qc).await;

        // Add the new vote to our aggregator and see if we have a quorum.
        if let Some(tc) = self.aggregator.add_timeout(timeout.clone())? {
            debug!("Assembled {:?}", tc);

            let timeout_committee = self
                .committees
                .get_committee_for_epoch(&self.epoch)
                .ok_or(ConsensusError::UnknownCommittee(self.epoch))?;

            // Try to advance the round.
            self.advance_round(
                tc.round,
                tc.epoch_concluded,
                tc.epoch,
                tc.last_snapshot.to_owned(),
            )
            .await;

            // Broadcast the TC.
            debug!("Broadcasting {:?}", tc);
            let addresses = timeout_committee
                .broadcast_addresses(&self.name)
                .into_iter()
                .map(|(_, x)| x)
                .collect();
            let message = bincode::serialize(&ConsensusMessage::TC(tc.clone()))
                .expect("Failed to serialize timeout certificate");
            self.network
                .broadcast(addresses, Bytes::from(message))
                .await;

            // Make a new block if we are the next leader.
            if self.name == self.leader_elector.get_leader(self.epoch, self.round) {
                let last_snapshot = tc.last_snapshot.to_owned();
                self.generate_proposal(Some(tc), last_snapshot).await;
            }
        }
        Ok(())
    }

    #[async_recursion]
    async fn advance_round(
        &mut self,
        round: Round,
        epoch_concluded: bool,
        epoch: EpochNumber,
        last_snapshot: Digest,
    ) {
        if epoch != self.epoch || round < self.round {
            return;
        }
        // Reset the timer and advance round.
        self.timer.reset();
        if epoch_concluded {
            self.round = 1;
            self.last_snapshot = None;
            self.update_epoch(&(epoch + 1)).await;
            debug!("Moved to epoch {} round {}", epoch + 1, self.round);
        } else {
            self.round = round + 1;
            self.last_snapshot = Some(last_snapshot);
            debug!("Moved to round {}", self.round);
        }

        // Cleanup the vote aggregator.
        self.aggregator.cleanup(&self.epoch, &self.round);
    }

    #[async_recursion]
    async fn generate_proposal(&mut self, tc: Option<TC>, last_snapshot: Digest) {
        if self
            .epoch_len
            .is_none_or(|epoch_len| self.round < epoch_len)
        {
            self.tx_proposer
                .send(ProposerMessage::Make(
                    self.epoch,
                    self.round,
                    self.high_qc.clone(),
                    tc,
                    if self.round == 1 {
                        Digest::default()
                    } else {
                        last_snapshot
                    },
                ))
                .await
                .expect("Failed to send message to proposer")
        } else if self
            .epoch_len
            .is_some_and(|epoch_len| epoch_len == self.round)
        {
            self.tx_proposer
                .send(ProposerMessage::MakeEpochChange(
                    self.epoch,
                    self.round,
                    self.high_qc.clone(),
                    tc,
                    last_snapshot,
                ))
                .await
                .expect("Failed to send message to proposer")
        } else if let Some(epoch_len) = self.epoch_len {
            unreachable!("Must not exceed round {} in any round", epoch_len)
        }
    }

    async fn cleanup_proposer(&mut self, b0: &Block, b1: &Block, block: &Block) {
        let digests = b0
            .payload
            .iter()
            .cloned()
            .chain(b1.payload.iter().cloned())
            .chain(block.payload.iter().cloned())
            .collect();
        self.tx_proposer
            .send(ProposerMessage::Cleanup(digests))
            .await
            .expect("Failed to send message to proposer");
    }

    async fn process_qc(&mut self, qc: &QC) {
        self.advance_round(
            qc.round,
            qc.epoch_concluded,
            qc.epoch,
            qc.last_snapshot.to_owned(),
        )
        .await;
        self.update_high_qc(qc);
    }

    #[async_recursion]
    async fn process_block(&mut self, block: &Block) -> ConsensusResult<()> {
        debug!("Processing Block {:?}", block);

        // Let's see if we have the last three ancestors of the block, that is:
        //      b0 <- |qc0; b1| <- |qc1; block|
        // If we don't, the synchronizer asks for them to other nodes. It will
        // then ensure we process both ancestors in the correct order, and
        // finally make us resume processing this block.
        let (b0, b1) = match self.synchronizer.get_ancestors(block).await? {
            Some(ancestors) => ancestors,
            None => {
                debug!("Processing of {} suspended: missing parent", block.digest());
                return Ok(());
            }
        };

        if b1.epoch_concluded {
            // Last block concluded last epoch, so current epoch must be old epoch + 1.
            ensure!(
                block.epoch == b1.epoch + 1,
                ConsensusError::MissingEpochBumpAfterEpochChange(b1.epoch)
            );
            ensure!(
                block.round == 1,
                ConsensusError::MissingRoundsResetAfterEpochChange(b1.epoch)
            );
            ensure!(
                block.snapshot == update_snapshot(&Digest::default(), &block.payload,),
                ConsensusError::InvalidSnapshot
            );
        } else {
            ensure!(
                block.snapshot == update_snapshot(&b1.snapshot, &block.payload,),
                ConsensusError::InvalidSnapshot
            );
        }

        // Store the block only if we have already processed all its ancestors.
        self.store_block(block).await;

        self.cleanup_proposer(&b0, &b1, block).await;

        // Check if we can commit the head of the 2-chain.
        // Note that we commit blocks only if we have all its ancestors.
        if is_subsequent(self.epoch_len, b0.epoch, b0.round, b1.epoch, b1.round) {
            self.mempool_driver.cleanup(b0.round).await;
            self.commit(b0).await?;
        }

        // Ensure the block's round is as expected.
        // This check is important: it prevents bad leaders from producing blocks
        // far in the future that may cause overflow on the round number.
        if block.round != self.round {
            return Ok(());
        }

        // See if we can vote for this block.
        if let Some(vote) = self.make_vote(block).await {
            debug!("Created {:?}", vote);
            let next_leader = self.leader_elector.get_leader(self.epoch, self.round + 1);
            if next_leader == self.name {
                self.handle_vote(&vote).await?;
            } else {
                let epoch_committee = self
                    .committees
                    .get_committee_for_epoch(&self.epoch)
                    .expect("Missing committee for epoch {epoch}");

                debug!("Sending {:?} to {}", vote, next_leader);
                let address = epoch_committee
                    .address(&next_leader)
                    .expect("The next leader is not in the committee");
                let message = bincode::serialize(&ConsensusMessage::Vote(vote))
                    .expect("Failed to serialize vote");
                self.network.send(address, Bytes::from(message)).await;
            }
        }
        Ok(())
    }

    async fn handle_proposal(&mut self, block: &Block) -> ConsensusResult<()> {
        let digest = block.digest();

        // Ensure the block proposer is the right leader for the round.
        ensure!(
            block.author == self.leader_elector.get_leader(block.epoch, block.round),
            ConsensusError::WrongLeader {
                digest,
                leader: block.author,
                round: block.round
            }
        );

        // Check the block is correctly formed.
        block.verify(&self.committees)?;

        // Process the QC. This may allow us to advance round.
        self.process_qc(&block.qc).await;

        // Process the TC (if any). This may also allow us to advance round.
        if let Some(ref tc) = block.tc {
            self.advance_round(
                tc.round,
                tc.epoch_concluded,
                tc.epoch,
                tc.last_snapshot.to_owned(),
            )
            .await;
        }

        // Let's see if we have the block's data. If we don't, the mempool
        // will get it and then make us resume processing this block.
        if !self.mempool_driver.verify(block.clone()).await? {
            debug!("Processing of {} suspended: missing payload", digest);
            return Ok(());
        }

        // All check pass, we can process this block.
        self.process_block(block).await
    }

    async fn handle_tc(&mut self, tc: TC) -> ConsensusResult<()> {
        tc.verify(&self.committees)?;
        if tc.epoch != self.epoch || tc.round < self.round {
            return Ok(());
        }
        self.advance_round(
            tc.round,
            tc.epoch_concluded,
            tc.epoch,
            tc.last_snapshot.to_owned(),
        )
        .await;
        if self.name == self.leader_elector.get_leader(self.epoch, self.round) {
            let last_snapshot = tc.last_snapshot.to_owned();
            self.generate_proposal(Some(tc), last_snapshot).await;
        }
        Ok(())
    }

    pub async fn run(&mut self) {
        // Upon booting, generate the very first block (if we are the leader).
        // Also, schedule a timer in case we don't hear from the leader.
        self.timer.reset();
        if self.name == self.leader_elector.get_leader(self.epoch, self.round) {
            self.generate_proposal(None, Digest::default()).await;
        }

        // This is the main loop: it processes incoming blocks and votes,
        // and receive timeout notifications from our Timeout Manager.
        loop {
            let result = tokio::select! {
                Some(message) = self.rx_message.recv() => match message {
                    ConsensusMessage::Propose(block) => self.handle_proposal(&block).await,
                    ConsensusMessage::Vote(vote) => self.handle_vote(&vote).await,
                    ConsensusMessage::Timeout(timeout) => self.handle_timeout(&timeout).await,
                    ConsensusMessage::TC(tc) => self.handle_tc(tc).await,
                    _ => panic!("Unexpected protocol message")
                },
                Some(block) = self.rx_loopback.recv() => self.process_block(&block).await,
                () = &mut self.timer => self.local_timeout_round().await,
            };
            match result {
                Ok(()) => (),
                Err(ConsensusError::StoreError(e)) => error!("{}", e),
                Err(ConsensusError::SerializationError(e)) => error!("Store corrupted. {}", e),
                Err(e) => warn!("{}", e),
            }
        }
    }
}
