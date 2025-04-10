# Epochs

Epochs partition the chain of blocks in chunks that can get batched together such that nodes joining the network can rely on repeated checkpoints, rather than downloading and replaying every block.

An epoch starts by increasing the epoch counter by one and resettting the round counter to one. Nodes that are part of the committee which got assigned to this epoch, should propose and vote on blocks. Once an epoch got concluded, the next committee takes over and is responsible for producing and validating blocks. There are multiple possible ways to initiate the conclusion of an epoch, such as dynamically using system smart contracts or by statically following a hard-coded scheme. To keep things simple, in this implementation, epoch changes happen after a CLI-configurable amount of blocks. Please note that at the moment, there is no consensus on the utilized parameters, which means that all nodes participating in the network need to be start with the same epoch length.

```json
// content of the parameters.json file
{
  "consensus": {
    // ...
    "epoch_len": 1024
    // ...
  }
}
```

Note that if the `epoch_len` entry is missing, the nodes do not perform or accept any epoch changes.

```sh
cargo run --bin node run --parameters=./parameters.json # --keys= ... --store=... --committee=...
```

The actual epoch change happens by the producer of the block which sets the added property `epoch_concluded` to `true` and thus signals to all other consensus nodes that this particular block has been the last block in this epoch. However, as the implementation does not support dynamically initiating epoch changes, other nodes will not vote for a block at the designated epoch change that does not come with the `epoch_concluded` property set to `true`. Once receiving the proposal by the block proposer, the nodes craft a `Vote` and send the vote the designated proposer of the next block. Note that the proposer is part of a different committee. The nodes thus need to know the upcoming committee in advance and further need to be able to figure out the proposer of the first block in the next epoch. The first proposer of the next interval then aggregates the votes by the nodes of the last epoch and forms a quorum certificate which will be then part of the upcoming next block. To mark the epoch change, the proposer increases the `epoch` counter by one and resets the `round` property to one.

# Committees

In each epoch, there is one committee that is supposed to participate in the consensus. Once taking over from a previous committee, the proposer of the first block needs to verify the quorum certificate of the last block from the previous epoch. Nodes thus need to know which nodes in the previous committee have been eligible to participate in the consensus. To prevent non-consensus nodes from picking a different fork of the chain, consensus nodes need to check the quorum certificate carefully when proposing and voting for the first block in the new epoch.

Although the architecture supports different committees for each epoch, the current implementation assumes that nodes cannot leave consensus, i.e. it is expected that each nodes participating in the consensus will also be part of the next committee. However, the size of the committees might increase.

# Fast sync

In the moment that the last block of an epoch has been committed by the consensus nodes, the consensus nodes can batch all transactions included in that epoch and form a snapshot on the state at the end of this epoch. Non-consensus nodes can then query at pre-defined times the consensus nodes for latest snapshots. Instead of replaying all blocks and transactions, the nodes can adopt the new state, which is supposed to require much less computation and much less RPC calls.

# Implementation tradeoffs

- Epochs are concluded after a fixed threshold.
- The epoch length must be set a startup.
- No dynamic epoch lengths
- Although the committees might differ, the current implementation expects that all nodes from the previous committee are also part of the next committee. (Nodes cannot leave the consensus.)

# Testing

There is a test suite that spins up four consensus nodes with a mempool and runs them for 20 seconds to produce blocks.

The script has been adapted to trigger an epoch change every `1024` rounds.

Preparation (requires installed Python and Cargo + Rust compiler)

```sh
cd hotstuff/benchmark
pip install -r requirements.txt
```

Actual end-to-end test
```sh
fab local
```

Sample output at the epoch change at round `1024`:
```
[2025-04-10T09:07:57.659Z DEBUG consensus::core] Moved to round 1024
[2025-04-10T09:07:57.659Z DEBUG consensus::core] Processing Block 9AXbZRE9+X3eh1Gc: B(OcN0HErkwvz/zHCw, epoch 1 round 1024, QC(laMq2o/6Q/ipenSH, 1023), round_concluded true, 0)
[2025-04-10T09:07:57.659Z DEBUG consensus::core] Committed nW/wRtFQA/+CWSs4: B(gVoeUm/d+xPY+793, epoch 1 round 1022, QC(GQhML0KiX4Bxiswf, 1021), round_concluded false, 0)
[2025-04-10T09:07:57.659Z DEBUG consensus::core] Created V(1+Z4DNhW35sZP8ac, 1024, 9AXbZRE9+X3eh1Gc)
[2025-04-10T09:07:57.659Z DEBUG consensus::core] Sending V(1+Z4DNhW35sZP8ac, 1024, 9AXbZRE9+X3eh1Gc) to dKcog9w1CfvpsV8Z
[2025-04-10T09:07:57.659Z DEBUG consensus::core] Moved to epoch 2 round 1
[2025-04-10T09:07:57.659Z DEBUG consensus::core] Processing Block u3KGNjASsmad78wx: B(dKcog9w1CfvpsV8Z, epoch 2 round 1, QC(9AXbZRE9+X3eh1Gc, 1024), round_concluded false, 0)
[2025-04-10T09:07:57.660Z DEBUG consensus::core] Committed laMq2o/6Q/ipenSH: B(1+Z4DNhW35sZP8ac, epoch 1 round 1023, QC(nW/wRtFQA/+CWSs4, 1022), round_concluded false, 0)
[2025-04-10T09:07:57.660Z DEBUG consensus::core] Created V(1+Z4DNhW35sZP8ac, 1, u3KGNjASsmad78wx)
[2025-04-10T09:07:57.660Z DEBUG consensus::core] Sending V(1+Z4DNhW35sZP8ac, 1, u3KGNjASsmad78wx) to gVoeUm/d+xPY+793
[2025-04-10T09:07:57.660Z DEBUG consensus::core] Moved to round 2
[2025-04-10T09:07:57.660Z DEBUG consensus::core] Processing Block huvVYQlT4e3JmYGD: B(gVoeUm/d+xPY+793, epoch 2 round 2, QC(u3KGNjASsmad78wx, 1), round_concluded false, 0)
[2025-04-10T09:07:57.660Z DEBUG consensus::core] Committed 9AXbZRE9+X3eh1Gc: B(OcN0HErkwvz/zHCw, epoch 1 round 1024, QC(laMq2o/6Q/ipenSH, 1023), round_concluded true, 0)
```
