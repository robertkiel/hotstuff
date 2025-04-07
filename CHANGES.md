# Epochs

Epochs mark the change from one consensus committee to another.

# Epoch change protocol

Proposer broadcasts epoch change message. The epoch change message contains the quorum certificate of the last block and the next epoch.

```
Epoch change:
- qc: quorum certificate previous block
- r: current round
- hash: hash of last block
- epoch: current Epoch
- next_epoch: next Epoch
```

Upon receiving the epoch change message, nodes start to vote on the epoch change message by sending a signed vote to the next leader.

The next leader collects votes and forms a qurom certificate of the epoch change block and creates a new block in the next epoch, i.e. by setting the epoch of created block to the next epoch as specified by the epoch change block, and broadcasts the block to all replicas.

Once seeing the quorum certificate of the epoch change block, replicas lock the epoch change and are thus ready to commit to the epoch change once seeing a quorum certificate for the new block.

# Why empty block?

Voting on the epoch change block that replicas will not accept any subsequent block in that particular epoch.

A new epoch may come with a different committee. If there were no epoch change block, the new committee would vote on the end of the previous epoch.

Receiving the quorum certificate of an epoch block means that the qualified majority decided to end the epoch. All blocks up to the specified hash can be downloaded in a batch (fast sync)
