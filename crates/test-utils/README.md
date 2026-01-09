# Test Utils

Integration test framework for node-reth crates.

## Quick Start

```rust,ignore
use base_reth_test_utils::TestHarness;

#[tokio::test]
async fn test_example() -> eyre::Result<()> {
    let harness = TestHarness::new().await?;

    // Build blocks
    harness.advance_chain(5).await?;

    // RPC calls
    let alice = &harness.accounts().alice;
    let balance = harness.provider().get_balance(alice.address).await?;

    // Flashblocks
    harness.send_flashblock(flashblock).await?;
    let pending = harness.flashblocks_state();

    Ok(())
}
```

## TestHarness

The main entry point. Bundles node, Engine API, flashblocks, and test accounts.

**Constructors:**
- `new()` - Standard harness
- `manual_canonical()` - Disable automatic canonical processing (for race condition tests)
- `with_launcher(fn)` - Custom node launcher

**Methods:**
- `provider()` - Alloy RootProvider for RPC
- `accounts()` - Test accounts (alice, bob, charlie, deployer)
- `advance_chain(n)` - Build N empty blocks
- `build_block_from_transactions(txs)` - Build block with transactions
- `send_flashblock(fb)` / `send_flashblocks(iter)` - Send flashblocks
- `flashblocks_state()` - Access pending state
- `blockchain_provider()` - Direct database access
- `latest_block()` - Get latest canonical block

## LocalNode

Lower-level access to the in-process node. Most tests should use `TestHarness` instead.

```rust,ignore
use base_reth_test_utils::LocalNode;

let node = LocalNode::new().await?;
let provider = node.provider()?;
let engine = node.engine_api()?;
node.send_flashblock(flashblock).await?;
```

**Constructors:**
- `new()` - Standard node
- `manual_canonical()` - Manual canonical processing
- `with_launcher(fn)` / `manual_canonical_with_launcher(fn)` - Custom launcher

## Test Accounts

Pre-funded Anvil-compatible accounts (10,000 ETH each):

| Name | Address |
|------|---------|
| Alice | `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` |
| Bob | `0x70997970C51812dc3A010C7d01b50e0d17dc79C8` |
| Charlie | `0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC` |
| Deployer | `0x90F79bf6EB2c4f870365E785982E1f101E93b906` |

## Usage

```toml
[dev-dependencies]
base-reth-test-utils.workspace = true
```
