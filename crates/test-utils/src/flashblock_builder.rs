//! Builder for constructing test flashblocks.

use alloy_consensus::{Receipt, Transaction};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, BlockNumber, Bytes, U256, map::foldhash::HashMap};
use alloy_rpc_types_engine::PayloadId;
use base_flashtypes::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, Flashblock, Metadata,
};
use op_alloy_consensus::OpDepositReceipt;
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};

use crate::{L1_BLOCK_INFO_DEPOSIT_TX, L1_BLOCK_INFO_DEPOSIT_TX_HASH};

/// Information about the parent block needed to construct a flashblock.
#[derive(Debug, Clone)]
pub struct ParentBlockInfo {
    /// The block number of the parent block.
    pub number: BlockNumber,
    /// The hash of the parent block.
    pub hash: B256,
    /// The gas limit of the parent block.
    pub gas_limit: u64,
    /// The timestamp of the parent block.
    pub timestamp: u64,
}

/// Builder for constructing test flashblocks.
///
/// This builder provides a fluent API for creating flashblocks for testing purposes.
/// It supports both base flashblocks (index 0) and delta flashblocks (index > 0).
#[derive(Debug)]
pub struct FlashblockBuilder {
    transactions: Vec<Bytes>,
    receipts: Option<HashMap<B256, OpReceipt>>,
    parent_block: ParentBlockInfo,
    canonical_block_number: Option<BlockNumber>,
    index: u64,
}

impl FlashblockBuilder {
    /// Create a new base flashblock builder (index 0) with the L1 block info deposit transaction.
    ///
    /// Base flashblocks are the first flashblock in a sequence and include the
    /// execution payload base with parent block information.
    pub fn new_base(parent_block: &ParentBlockInfo) -> Self {
        Self {
            canonical_block_number: None,
            transactions: vec![L1_BLOCK_INFO_DEPOSIT_TX.clone()],
            receipts: Some({
                let mut receipts = HashMap::default();
                receipts.insert(
                    L1_BLOCK_INFO_DEPOSIT_TX_HASH,
                    OpReceipt::Deposit(OpDepositReceipt {
                        inner: Receipt {
                            status: true.into(),
                            cumulative_gas_used: 10000,
                            logs: vec![],
                        },
                        deposit_nonce: Some(4012991u64),
                        deposit_receipt_version: None,
                    }),
                );
                receipts
            }),
            index: 0,
            parent_block: parent_block.clone(),
        }
    }

    /// Create a new delta flashblock builder with the given index.
    ///
    /// Delta flashblocks (index > 0) contain additional transactions that extend
    /// the base flashblock. They do not include the execution payload base.
    pub fn new(parent_block: &ParentBlockInfo, index: u64) -> Self {
        Self {
            canonical_block_number: None,
            transactions: Vec::new(),
            receipts: Some(HashMap::default()),
            parent_block: parent_block.clone(),
            index,
        }
    }

    /// Set the receipts for this flashblock.
    ///
    /// Pass `None` to simulate flashblocks without receipts (useful for testing
    /// scenarios where receipts are optional).
    pub fn with_receipts(mut self, receipts: Option<HashMap<B256, OpReceipt>>) -> Self {
        self.receipts = receipts;
        self
    }

    /// Add transactions to this flashblock.
    ///
    /// This automatically generates success receipts for each transaction.
    /// Can only be called on delta flashblocks (index > 0).
    ///
    /// # Panics
    ///
    /// Panics if called on a base flashblock (index == 0).
    pub fn with_transactions(mut self, transactions: Vec<OpTransactionSigned>) -> Self {
        assert_ne!(self.index, 0, "Cannot set transactions for initial flashblock");
        self.transactions.clear();

        let mut cumulative_gas_used = 0;
        for txn in transactions.iter() {
            cumulative_gas_used += txn.gas_limit();
            self.transactions.push(txn.encoded_2718().into());
            if let Some(ref mut receipts) = self.receipts {
                receipts.insert(
                    B256::from(*txn.tx_hash()),
                    OpReceipt::Eip1559(Receipt {
                        status: true.into(),
                        cumulative_gas_used,
                        logs: vec![],
                    }),
                );
            }
        }
        self
    }

    /// Add raw transaction bytes to this flashblock.
    ///
    /// Unlike `with_transactions`, this does not generate receipts automatically.
    /// Can only be called on delta flashblocks (index > 0).
    ///
    /// # Panics
    ///
    /// Panics if called on a base flashblock (index == 0).
    pub fn with_raw_transactions(mut self, transactions: Vec<Bytes>) -> Self {
        assert_ne!(self.index, 0, "Cannot set transactions for initial flashblock");
        self.transactions = transactions;
        self
    }

    /// Override the canonical block number for this flashblock.
    ///
    /// If not set, the block number defaults to parent block number + 1.
    pub fn with_canonical_block_number(mut self, num: BlockNumber) -> Self {
        self.canonical_block_number = Some(num);
        self
    }

    /// Build the flashblock.
    pub fn build(self) -> Flashblock {
        let canonical_block_num =
            self.canonical_block_number.unwrap_or(self.parent_block.number) + 1;

        let base = if self.index == 0 {
            Some(ExecutionPayloadBaseV1 {
                parent_beacon_block_root: self.parent_block.hash,
                parent_hash: self.parent_block.hash,
                fee_recipient: Address::random(),
                prev_randao: B256::random(),
                block_number: canonical_block_num,
                gas_limit: self.parent_block.gas_limit,
                timestamp: self.parent_block.timestamp + 2,
                extra_data: Bytes::new(),
                base_fee_per_gas: U256::from(100),
            })
        } else {
            None
        };

        Flashblock {
            payload_id: PayloadId::default(),
            index: self.index,
            base,
            diff: ExecutionPayloadFlashblockDeltaV1 {
                state_root: B256::default(),
                receipts_root: B256::default(),
                block_hash: B256::default(),
                gas_used: 0,
                withdrawals: Vec::new(),
                logs_bloom: Default::default(),
                withdrawals_root: Default::default(),
                transactions: self.transactions,
                blob_gas_used: Default::default(),
            },
            metadata: Metadata { block_number: canonical_block_num },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_flashblock_includes_deposit_tx() {
        let parent =
            ParentBlockInfo { number: 0, hash: B256::ZERO, gas_limit: 30_000_000, timestamp: 0 };

        let flashblock = FlashblockBuilder::new_base(&parent).build();

        assert_eq!(flashblock.index, 0);
        assert!(flashblock.base.is_some());
        assert_eq!(flashblock.diff.transactions.len(), 1);
        assert_eq!(flashblock.metadata.block_number, 1);
    }

    #[test]
    fn test_delta_flashblock_no_base() {
        let parent =
            ParentBlockInfo { number: 0, hash: B256::ZERO, gas_limit: 30_000_000, timestamp: 0 };

        let flashblock = FlashblockBuilder::new(&parent, 1).build();

        assert_eq!(flashblock.index, 1);
        assert!(flashblock.base.is_none());
        assert!(flashblock.diff.transactions.is_empty());
    }

    #[test]
    fn test_with_canonical_block_number() {
        let parent =
            ParentBlockInfo { number: 0, hash: B256::ZERO, gas_limit: 30_000_000, timestamp: 0 };

        let flashblock =
            FlashblockBuilder::new_base(&parent).with_canonical_block_number(100).build();

        assert_eq!(flashblock.metadata.block_number, 101);
        assert_eq!(flashblock.base.as_ref().unwrap().block_number, 101);
    }
}
