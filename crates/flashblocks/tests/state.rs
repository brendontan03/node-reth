//! Integration tests that stress Flashblocks state handling.

use std::sync::Arc;

use alloy_eips::BlockHashOrNumber;
use alloy_primitives::U256;
use base_reth_flashblocks::{FlashblocksAPI, FlashblocksState, PendingBlocksAPI};
use base_reth_test_utils::{FlashblockBuilder, LocalNodeProvider, TestHarness, User};
use op_alloy_network::BlockResponse;
use reth::providers::{AccountReader, BlockReader};
use reth_primitives_traits::{Account, Block as BlockT};
use reth_provider::StateProviderFactory;

// The amount of time to wait (in milliseconds) after sending a new flashblock or canonical block
// so it can be processed by the state processor
const SLEEP_TIME: u64 = 10;

/// Test-specific helpers that extend the base TestHarness for state tests.
struct StateTest {
    harness: TestHarness,
    flashblocks: Arc<FlashblocksState<LocalNodeProvider>>,
    provider: LocalNodeProvider,
}

impl StateTest {
    async fn new() -> Self {
        // These tests simulate pathological timing (missing receipts, reorgs, etc.), so we disable
        // the automatic canonical listener and only apply blocks when the test explicitly requests it.
        let harness =
            TestHarness::manual_canonical().await.expect("able to launch flashblocks harness");
        let provider = harness.blockchain_provider();
        let flashblocks = harness.flashblocks_state();

        let genesis_block = provider
            .block(BlockHashOrNumber::Number(0))
            .expect("able to load block")
            .expect("block exists")
            .try_into_recovered()
            .expect("able to recover block");
        flashblocks.on_canonical_block_received(genesis_block);

        Self { harness, flashblocks, provider }
    }

    fn canonical_account(&self, u: User) -> Account {
        self.provider
            .basic_account(&self.harness.address(u))
            .expect("can lookup account state")
            .expect("should be existing account state")
    }

    fn canonical_balance(&self, u: User) -> U256 {
        self.canonical_account(u).balance
    }

    fn expected_pending_balance(&self, u: User, delta: u128) -> U256 {
        self.canonical_balance(u) + U256::from(delta)
    }

    /// Get the pending nonce for a user (canonical + pending transaction count).
    fn pending_nonce(&self, u: User) -> u64 {
        let basic_account = self.canonical_account(u);
        let pending_count = self
            .flashblocks
            .get_pending_blocks()
            .get_transaction_count(self.harness.address(u))
            .to::<u64>();
        basic_account.nonce + pending_count
    }
}

impl std::ops::Deref for StateTest {
    type Target = TestHarness;

    fn deref(&self) -> &Self::Target {
        &self.harness
    }
}

#[tokio::test]
async fn test_state_overrides_persisted_across_flashblocks() {
    let test = StateTest::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build())
        .await
        .unwrap();
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
            .with_transactions(vec![test.build_eth_transfer(
                User::Alice,
                User::Bob,
                100_000,
                test.pending_nonce(User::Alice),
            )])
            .build(),
    )
    .await
    .unwrap();

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

    test.send_flashblock(FlashblockBuilder::new(&test.parent_block_info(), 2).build())
        .await
        .unwrap();

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
    let test = StateTest::new().await;

    let initial_base = FlashblockBuilder::new_base(&test.parent_block_info()).build();
    let initial_block_number = initial_base.metadata.block_number;
    test.send_flashblock(initial_base).await.unwrap();
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
            .with_transactions(vec![test.build_eth_transfer(
                User::Alice,
                User::Bob,
                100_000,
                test.pending_nonce(User::Alice),
            )])
            .build(),
    )
    .await
    .unwrap();

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
    .await
    .unwrap();

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
            .with_transactions(vec![test.build_eth_transfer(
                User::Alice,
                User::Bob,
                100_000,
                test.pending_nonce(User::Alice),
            )])
            .build(),
    )
    .await
    .unwrap();

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
    let test = StateTest::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build())
        .await
        .unwrap();
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
            .with_transactions(vec![test.build_eth_transfer(
                User::Alice,
                User::Bob,
                100_000,
                test.pending_nonce(User::Alice),
            )])
            .build(),
    )
    .await
    .unwrap();
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
    .await
    .unwrap();
    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_canonical_block_number(1)
            .with_transactions(vec![test.build_eth_transfer(
                User::Alice,
                User::Bob,
                100_000,
                test.pending_nonce(User::Alice),
            )])
            .build(),
    )
    .await
    .unwrap();
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

    test.new_canonical_block(
        vec![test.build_eth_transfer(User::Alice, User::Bob, 100, 0)],
        SLEEP_TIME,
    )
    .await
    .unwrap();

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
    let test = StateTest::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build())
        .await
        .unwrap();
    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_transactions(vec![test.build_eth_transfer(
                User::Alice,
                User::Bob,
                100,
                test.pending_nonce(User::Alice),
            )])
            .build(),
    )
    .await
    .unwrap();

    let pending_nonce =
        test.provider.basic_account(&test.address(User::Alice)).unwrap().unwrap().nonce
            + test
                .flashblocks
                .get_pending_blocks()
                .get_transaction_count(test.address(User::Alice))
                .to::<u64>();
    assert_eq!(pending_nonce, 1);

    test.new_canonical_block_without_processing(vec![test.build_eth_transfer(
        User::Alice,
        User::Bob,
        100,
        0,
    )])
    .await
    .unwrap();

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
    let test = StateTest::new().await;

    // Send a flashblock with no receipts (only deposit transaction)
    test.send_flashblock(
        FlashblockBuilder::new_base(&test.parent_block_info()).with_receipts(None).build(),
    )
    .await
    .unwrap();

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
    let test = StateTest::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build())
        .await
        .unwrap();

    let current_block =
        test.flashblocks.get_pending_blocks().get_block(true).expect("should be a block");

    assert_eq!(current_block.header().number, 1);
    assert_eq!(current_block.transactions.len(), 1);

    test.send_flashblock(
        FlashblockBuilder::new(&test.parent_block_info(), 1)
            .with_canonical_block_number(100)
            .build(),
    )
    .await
    .unwrap();

    let current_block = test.flashblocks.get_pending_blocks().get_block(true);
    assert!(current_block.is_none());
}

#[tokio::test]
async fn test_flashblock_for_new_canonical_block_works_if_sequential() {
    let test = StateTest::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build())
        .await
        .unwrap();

    let current_block =
        test.flashblocks.get_pending_blocks().get_block(true).expect("should be a block");

    assert_eq!(current_block.header().number, 1);
    assert_eq!(current_block.transactions.len(), 1);

    test.send_flashblock(
        FlashblockBuilder::new_base(&test.parent_block_info())
            .with_canonical_block_number(1)
            .build(),
    )
    .await
    .unwrap();

    let current_block =
        test.flashblocks.get_pending_blocks().get_block(true).expect("should be a block");

    assert_eq!(current_block.header().number, 2);
    assert_eq!(current_block.transactions.len(), 1);
}

#[tokio::test]
async fn test_non_sequential_payload_clears_pending_state() {
    let test = StateTest::new().await;

    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build())
        .await
        .unwrap();

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
            .with_transactions(vec![test.build_eth_transfer(
                User::Alice,
                User::Bob,
                100,
                test.pending_nonce(User::Alice),
            )])
            .build(),
    )
    .await
    .unwrap();

    assert_eq!(test.flashblocks.get_pending_blocks().is_none(), true);
}

#[tokio::test]
async fn test_duplicate_flashblock_ignored() {
    let test = StateTest::new().await;

    test.send_flashblock(FlashblockBuilder::new_base(&test.parent_block_info()).build())
        .await
        .unwrap();

    let fb = FlashblockBuilder::new(&test.parent_block_info(), 1)
        .with_transactions(vec![test.build_eth_transfer(
            User::Alice,
            User::Bob,
            100_000,
            test.pending_nonce(User::Alice),
        )])
        .build();

    test.send_flashblock(fb.clone()).await.unwrap();
    let block = test.flashblocks.get_pending_blocks().get_block(true);

    test.send_flashblock(fb.clone()).await.unwrap();
    let block_two = test.flashblocks.get_pending_blocks().get_block(true);

    assert_eq!(block, block_two);
}

#[tokio::test]
async fn test_progress_canonical_blocks_without_flashblocks() {
    let test = StateTest::new().await;

    let genesis_block = test.latest_block();
    assert_eq!(genesis_block.number, 0);
    assert_eq!(genesis_block.transaction_count(), 0);
    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());

    test.new_canonical_block(
        vec![test.build_eth_transfer(User::Alice, User::Bob, 100, test.pending_nonce(User::Alice))],
        SLEEP_TIME,
    )
    .await
    .unwrap();

    let block_one = test.latest_block();
    assert_eq!(block_one.number, 1);
    assert_eq!(block_one.transaction_count(), 2);
    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());

    test.new_canonical_block(
        vec![
            test.build_eth_transfer(User::Bob, User::Charlie, 100, test.pending_nonce(User::Bob)),
            test.build_eth_transfer(
                User::Charlie,
                User::Alice,
                1000,
                test.pending_nonce(User::Charlie),
            ),
        ],
        SLEEP_TIME,
    )
    .await
    .unwrap();

    let block_two = test.latest_block();
    assert_eq!(block_two.number, 2);
    assert_eq!(block_two.transaction_count(), 3);
    assert!(test.flashblocks.get_pending_blocks().get_block(true).is_none());
}
