//! Integration tests that stress Flashblocks state handling.

use std::sync::Arc;

use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{Address, U256, map::foldhash::HashMap};
use base_flashtypes::Flashblock;
use base_reth_flashblocks::{FlashblocksAPI, FlashblocksState, PendingBlocksAPI};
use base_reth_test_utils::{
    FlashblockBuilder, LocalNodeProvider, ParentBlockInfo, TestHarness as BaseTestHarness,
};
use op_alloy_network::BlockResponse;
use reth::{
    chainspec::EthChainSpec,
    providers::{AccountReader, BlockReader},
};
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives_traits::{Account, Block as BlockT};
use reth_provider::{ChainSpecProvider, StateProviderFactory};

// The amount of time to wait (in milliseconds) after sending a new flashblock or canonical block
// so it can be processed by the state processor
const SLEEP_TIME: u64 = 10;

#[derive(Eq, PartialEq, Debug, Hash, Clone, Copy)]
enum User {
    Alice,
    Bob,
    Charlie,
}

struct TestHarness {
    node: BaseTestHarness,
    flashblocks: Arc<FlashblocksState<LocalNodeProvider>>,
    provider: LocalNodeProvider,
    user_to_address: HashMap<User, Address>,
}

impl TestHarness {
    async fn new() -> Self {
        // These tests simulate pathological timing (missing receipts, reorgs, etc.), so we disable
        // the automatic canonical listener and only apply blocks when the test explicitly requests it.
        let node =
            BaseTestHarness::manual_canonical().await.expect("able to launch flashblocks harness");
        let provider = node.blockchain_provider();
        let flashblocks = node.flashblocks_state();

        let genesis_block = provider
            .block(BlockHashOrNumber::Number(0))
            .expect("able to load block")
            .expect("block exists")
            .try_into_recovered()
            .expect("able to recover block");
        flashblocks.on_canonical_block_received(genesis_block);

        let accounts = node.accounts().clone();

        let mut user_to_address = HashMap::default();
        user_to_address.insert(User::Alice, accounts.alice.address);
        user_to_address.insert(User::Bob, accounts.bob.address);
        user_to_address.insert(User::Charlie, accounts.charlie.address);

        Self { node, flashblocks, provider, user_to_address }
    }

    fn address(&self, u: User) -> Address {
        assert!(self.user_to_address.contains_key(&u));
        self.user_to_address[&u]
    }

    fn account(&self, u: User) -> &base_reth_test_utils::Account {
        match u {
            User::Alice => &self.node.accounts().alice,
            User::Bob => &self.node.accounts().bob,
            User::Charlie => &self.node.accounts().charlie,
        }
    }

    fn canonical_account(&self, u: User) -> Account {
        self.provider
            .basic_account(&self.address(u))
            .expect("can lookup account state")
            .expect("should be existing account state")
    }

    fn canonical_balance(&self, u: User) -> U256 {
        self.canonical_account(u).balance
    }

    fn expected_pending_balance(&self, u: User, delta: u128) -> U256 {
        self.canonical_balance(u) + U256::from(delta)
    }

    fn account_state(&self, u: User) -> Account {
        let basic_account = self.canonical_account(u);

        let nonce = self
            .flashblocks
            .get_pending_blocks()
            .get_transaction_count(self.address(u))
            .to::<u64>();
        let balance = self
            .flashblocks
            .get_pending_blocks()
            .get_balance(self.address(u))
            .unwrap_or(basic_account.balance);

        Account {
            nonce: nonce + basic_account.nonce,
            balance,
            bytecode_hash: basic_account.bytecode_hash,
        }
    }

    fn chain_id(&self) -> u64 {
        self.provider.chain_spec().chain_id()
    }

    fn parent_block_info(&self) -> ParentBlockInfo {
        self.node.parent_block_info()
    }

    fn build_transaction_to_send_eth(
        &self,
        from: User,
        to: User,
        amount: u128,
    ) -> OpTransactionSigned {
        self.account(from).build_eth_transfer(
            self.address(to),
            amount,
            self.account_state(from).nonce,
            self.chain_id(),
        )
    }

    fn build_transaction_to_send_eth_with_nonce(
        &self,
        from: User,
        to: User,
        amount: u128,
        nonce: u64,
    ) -> OpTransactionSigned {
        self.account(from).build_eth_transfer(self.address(to), amount, nonce, self.chain_id())
    }

    async fn send_flashblock(&self, flashblock: Flashblock) {
        self.node
            .send_flashblock_and_wait(flashblock, SLEEP_TIME)
            .await
            .expect("flashblocks channel should accept payload");
    }

    async fn new_canonical_block_without_processing(&self, transactions: Vec<OpTransactionSigned>) {
        self.node
            .new_canonical_block_without_processing(transactions)
            .await
            .expect("able to build canonical block");
    }

    async fn new_canonical_block(&self, transactions: Vec<OpTransactionSigned>) {
        self.node
            .new_canonical_block(transactions, SLEEP_TIME)
            .await
            .expect("able to build canonical block");
    }
}

#[tokio::test]
async fn test_state_overrides_persisted_across_flashblocks() {
    let test = TestHarness::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build()).await;
    assert_eq!(
        test.flashblocks
            .get_pending_blocks()
            .get_block(true)
            .expect("block is built")
            .transactions
            .len(),
        1
    );

    assert!(test.flashblocks.get_pending_blocks().get_state_overrides().is_some());
    assert!(
        !test
            .flashblocks
            .get_pending_blocks()
            .get_state_overrides()
            .unwrap()
            .contains_key(&test.address(User::Alice))
    );

    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_transactions(vec![test.build_transaction_to_send_eth(
                User::Alice,
                User::Bob,
                100_000,
            )])
            .build(),
    )
    .await;

    let pending = test.flashblocks.get_pending_blocks().get_block(true);
    assert!(pending.is_some());
    let pending = pending.unwrap();
    assert_eq!(pending.transactions.len(), 2);

    let overrides = test
        .flashblocks
        .get_pending_blocks()
        .get_state_overrides()
        .expect("should be set from txn execution");

    assert!(overrides.get(&test.address(User::Alice)).is_some());
    assert_eq!(
        overrides
            .get(&test.address(User::Bob))
            .expect("should be set as txn receiver")
            .balance
            .expect("should be changed due to receiving funds"),
        test.expected_pending_balance(User::Bob, 100_000)
    );

    test.send_flashblock(FlashblockBuilder::new(&test.parent_block_info(), 2).build()).await;

    let overrides = test
        .flashblocks
        .get_pending_blocks()
        .get_state_overrides()
        .expect("should be set from txn execution in flashblock index 1");

    assert!(overrides.get(&test.address(User::Alice)).is_some());
    assert_eq!(
        overrides
            .get(&test.address(User::Bob))
            .expect("should be set as txn receiver")
            .balance
            .expect("should be changed due to receiving funds"),
        test.expected_pending_balance(User::Bob, 100_000)
    );
}

#[tokio::test]
async fn test_state_overrides_persisted_across_blocks() {
    let test = TestHarness::new().await;

    let initial_base = FlashblockBuilder::new_base(&test.parent_block_info()).build();
    let initial_block_number = initial_base.metadata.block_number;
    test.send_flashblock(initial_base).await;
    assert_eq!(
        test.flashblocks
            .get_pending_blocks()
            .get_block(true)
            .expect("block is built")
            .transactions
            .len(),
        1
    );

    assert!(test.flashblocks.get_pending_blocks().get_state_overrides().is_some());
    assert!(
        !test
            .flashblocks
            .get_pending_blocks()
            .get_state_overrides()
            .unwrap()
            .contains_key(&test.address(User::Alice))
    );

    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_transactions(vec![test.build_transaction_to_send_eth(
                User::Alice,
                User::Bob,
                100_000,
            )])
            .build(),
    )
    .await;

    let pending = test.flashblocks.get_pending_blocks().get_block(true);
    assert!(pending.is_some());
    let pending = pending.unwrap();
    assert_eq!(pending.transactions.len(), 2);

    let overrides = test
        .flashblocks
        .get_pending_blocks()
        .get_state_overrides()
        .expect("should be set from txn execution");

    assert!(overrides.get(&test.address(User::Alice)).is_some());
    assert_eq!(
        overrides
            .get(&test.address(User::Bob))
            .expect("should be set as txn receiver")
            .balance
            .expect("should be changed due to receiving funds"),
        test.expected_pending_balance(User::Bob, 100_000)
    );

    test.send_flashblock(
        FlashblockBuilder::new_base(&test.parent_block_info())
            .with_canonical_block_number(initial_block_number)
            .build(),
    )
    .await;

    assert_eq!(
        test.flashblocks
            .get_pending_blocks()
            .get_block(true)
            .expect("block is built")
            .transactions
            .len(),
        1
    );
    assert_eq!(
        test.flashblocks
            .get_pending_blocks()
            .get_block(true)
            .expect("block is built")
            .header
            .number,
        initial_block_number + 1
    );

    assert!(test.flashblocks.get_pending_blocks().get_state_overrides().is_some());
    assert!(
        test.flashblocks
            .get_pending_blocks()
            .get_state_overrides()
            .unwrap()
            .contains_key(&test.address(User::Alice))
    );

    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_canonical_block_number(initial_block_number)
            .with_transactions(vec![test.build_transaction_to_send_eth(
                User::Alice,
                User::Bob,
                100_000,
            )])
            .build(),
    )
    .await;

    let overrides = test
        .flashblocks
        .get_pending_blocks()
        .get_state_overrides()
        .expect("should be set from txn execution");

    assert!(overrides.get(&test.address(User::Alice)).is_some());
    assert_eq!(
        overrides
            .get(&test.address(User::Bob))
            .expect("should be set as txn receiver")
            .balance
            .expect("should be changed due to receiving funds"),
        test.expected_pending_balance(User::Bob, 200_000)
    );
}

#[tokio::test]
async fn test_only_current_pending_state_cleared_upon_canonical_block_reorg() {
    let test = TestHarness::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build()).await;
    assert_eq!(
        test.flashblocks
            .get_pending_blocks()
            .get_block(true)
            .expect("block is built")
            .transactions
            .len(),
        1
    );
    assert!(test.flashblocks.get_pending_blocks().get_state_overrides().is_some());
    assert!(
        !test
            .flashblocks
            .get_pending_blocks()
            .get_state_overrides()
            .unwrap()
            .contains_key(&test.address(User::Alice))
    );

    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_transactions(vec![test.build_transaction_to_send_eth(
                User::Alice,
                User::Bob,
                100_000,
            )])
            .build(),
    )
    .await;
    let pending = test.flashblocks.get_pending_blocks().get_block(true);
    assert!(pending.is_some());
    let pending = pending.unwrap();
    assert_eq!(pending.transactions.len(), 2);

    let overrides = test
        .flashblocks
        .get_pending_blocks()
        .get_state_overrides()
        .expect("should be set from txn execution");

    assert!(overrides.get(&test.address(User::Alice)).is_some());
    assert_eq!(
        overrides
            .get(&test.address(User::Bob))
            .expect("should be set as txn receiver")
            .balance
            .expect("should be changed due to receiving funds"),
        test.expected_pending_balance(User::Bob, 100_000)
    );

    test.send_flashblock(
        FlashblockBuilder::new_base(&test.parent_block_info())
            .with_canonical_block_number(1)
            .build(),
    )
    .await;
    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_canonical_block_number(1)
            .with_transactions(vec![test.build_transaction_to_send_eth(
                User::Alice,
                User::Bob,
                100_000,
            )])
            .build(),
    )
    .await;
    let pending = test.flashblocks.get_pending_blocks().get_block(true);
    assert!(pending.is_some());
    let pending = pending.unwrap();
    assert_eq!(pending.transactions.len(), 2);

    let overrides = test
        .flashblocks
        .get_pending_blocks()
        .get_state_overrides()
        .expect("should be set from txn execution");

    assert!(overrides.get(&test.address(User::Alice)).is_some());
    assert_eq!(
        overrides
            .get(&test.address(User::Bob))
            .expect("should be set as txn receiver")
            .balance
            .expect("should be changed due to receiving funds"),
        test.expected_pending_balance(User::Bob, 200_000)
    );

    test.new_canonical_block(vec![test.build_transaction_to_send_eth_with_nonce(
        User::Alice,
        User::Bob,
        100,
        0,
    )])
    .await;

    let pending = test.flashblocks.get_pending_blocks().get_block(true);
    assert!(pending.is_some());
    let pending = pending.unwrap();
    assert_eq!(pending.transactions.len(), 2);

    let overrides = test
        .flashblocks
        .get_pending_blocks()
        .get_state_overrides()
        .expect("should be set from txn execution");

    assert!(overrides.get(&test.address(User::Alice)).is_some());
    assert_eq!(
        overrides
            .get(&test.address(User::Bob))
            .expect("should be set as txn receiver")
            .balance
            .expect("should be changed due to receiving funds"),
        test.expected_pending_balance(User::Bob, 100_000)
    );
}

#[tokio::test]
async fn test_nonce_uses_pending_canon_block_instead_of_latest() {
    // Test for race condition when a canon block comes in but user
    // requests their nonce prior to the StateProcessor processing the canon block
    // causing it to return an n+1 nonce instead of n
    // because underlying reth node `latest` block is already updated, but
    // relevant pending state has not been cleared yet
    let test = TestHarness::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build()).await;
    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_transactions(vec![test.build_transaction_to_send_eth(
                User::Alice,
                User::Bob,
                100,
            )])
            .build(),
    )
    .await;

    let pending_nonce =
        test.provider.basic_account(&test.address(User::Alice)).unwrap().unwrap().nonce
            + test
                .flashblocks
                .get_pending_blocks()
                .get_transaction_count(test.address(User::Alice))
                .to::<u64>();
    assert_eq!(pending_nonce, 1);

    test.new_canonical_block_without_processing(vec![
        test.build_transaction_to_send_eth_with_nonce(User::Alice, User::Bob, 100, 0),
    ])
    .await;

    let pending_nonce =
        test.provider.basic_account(&test.address(User::Alice)).unwrap().unwrap().nonce
            + test
                .flashblocks
                .get_pending_blocks()
                .get_transaction_count(test.address(User::Alice))
                .to::<u64>();

    // This is 2, because canon block has reached the underlying chain
    // but the StateProcessor hasn't processed it
    // so pending nonce is effectively double-counting the same transaction, leading to a nonce of 2
    assert_eq!(pending_nonce, 2);

    // On the RPC level, we correctly return 1 because we
    // use the pending canon block instead of the latest block when fetching
    // onchain nonce count to compute
    // pending_nonce = onchain_nonce + pending_txn_count
    let canon_block = test.flashblocks.get_pending_blocks().get_canonical_block_number();
    let canon_state_provider = test.provider.state_by_block_number_or_tag(canon_block).unwrap();
    let canon_nonce =
        canon_state_provider.account_nonce(&test.address(User::Alice)).unwrap().unwrap();
    let pending_nonce = canon_nonce
        + test
            .flashblocks
            .get_pending_blocks()
            .get_transaction_count(test.address(User::Alice))
            .to::<u64>();
    assert_eq!(pending_nonce, 1);
}

#[tokio::test]
async fn test_metadata_receipts_are_optional() {
    // Test to ensure that receipts are optional in the metadata
    // and deposit receipts return None for nonce until the canonical block is processed
    let test = TestHarness::new().await;

    // Send a flashblock with no receipts (only deposit transaction)
    test.send_flashblock(
        FlashblockBuilder::new_base(&test.parent_block_info()).with_receipts(None).build(),
    )
    .await;

    // Verify the block was created with the deposit transaction
    let pending_block =
        test.flashblocks.get_pending_blocks().get_block(true).expect("block should be created");
    assert_eq!(pending_block.transactions.len(), 1);

    // Check that the deposit transaction has the correct nonce
    let deposit_tx = &pending_block.transactions.as_transactions().unwrap()[0];
    assert_eq!(
        deposit_tx.deposit_nonce,
        Some(0),
        "deposit_nonce should be available even when no receipts"
    );
}

#[tokio::test]
async fn test_flashblock_for_new_canonical_block_clears_older_flashblocks_if_non_zero_index() {
    let test = TestHarness::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build()).await;

    let current_block =
        test.flashblocks.get_pending_blocks().get_block(true).expect("should be a block");

    assert_eq!(current_block.header().number, 1);
    assert_eq!(current_block.transactions.len(), 1);

    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_canonical_block_number(100)
            .build(),
    )
    .await;

    let current_block = test.flashblocks.get_pending_blocks().get_block(true);
    assert!(current_block.is_none());
}

#[tokio::test]
async fn test_flashblock_for_new_canonical_block_works_if_sequential() {
    let test = TestHarness::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build()).await;

    let current_block =
        test.flashblocks.get_pending_blocks().get_block(true).expect("should be a block");

    assert_eq!(current_block.header().number, 1);
    assert_eq!(current_block.transactions.len(), 1);

    test.send_flashblock(
        FlashblockBuilder::new_base(&test.parent_block_info())
            .with_canonical_block_number(1)
            .build(),
    )
    .await;

    let current_block =
        test.flashblocks.get_pending_blocks().get_block(true).expect("should be a block");

    assert_eq!(current_block.header().number, 2);
    assert_eq!(current_block.transactions.len(), 1);
}

#[tokio::test]
async fn test_non_sequential_payload_clears_pending_state() {
    let test = TestHarness::new().await;

    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build()).await;

    // Just the block info transaction
    assert_eq!(
        test.flashblocks
            .get_pending_blocks()
            .get_block(true)
            .expect("should be set")
            .transactions
            .len(),
        1
    );

    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 3)
            .with_transactions(vec![test.build_transaction_to_send_eth(
                User::Alice,
                User::Bob,
                100,
            )])
            .build(),
    )
    .await;

    assert_eq!(test.flashblocks.get_pending_blocks().is_none(), true);
}

#[tokio::test]
async fn test_duplicate_flashblock_ignored() {
    let test = TestHarness::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build()).await;

    let fb = FlashblockBuilder::new(&test.parent_block_info(), 1)
        .with_transactions(vec![test.build_transaction_to_send_eth(
            User::Alice,
            User::Bob,
            100_000,
        )])
        .build();

    test.send_flashblock(fb.clone()).await;
    let block = test.flashblocks.get_pending_blocks().get_block(true);

    test.send_flashblock(fb.clone()).await;
    let block_two = test.flashblocks.get_pending_blocks().get_block(true);

    assert_eq!(block, block_two);
}

#[tokio::test]
async fn test_progress_canonical_blocks_without_flashblocks() {
    let test = TestHarness::new().await;

    let genesis_block = test.node.latest_block();
    assert_eq!(genesis_block.number, 0);
    assert_eq!(genesis_block.transaction_count(), 0);
    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());

    test.new_canonical_block(vec![test.build_transaction_to_send_eth(User::Alice, User::Bob, 100)])
        .await;

    let block_one = test.node.latest_block();
    assert_eq!(block_one.number, 1);
    assert_eq!(block_one.transaction_count(), 2);
    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());

    test.new_canonical_block(vec![
        test.build_transaction_to_send_eth(User::Bob, User::Charlie, 100),
        test.build_transaction_to_send_eth(User::Charlie, User::Alice, 1000),
    ])
    .await;

    let block_two = test.node.latest_block();
    assert_eq!(block_two.number, 2);
    assert_eq!(block_two.transaction_count(), 3);
    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());
}
