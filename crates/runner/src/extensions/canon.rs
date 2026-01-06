//! Contains the [FlashblocksCanonExtension] which wires up the `flashblocks-canon`
//! execution extension on the Base node builder.

use std::sync::Arc;

use base_reth_flashblocks::FlashblocksState;
use futures_util::TryStreamExt;
use reth_exex::{ExExEvent, ExExNotification};
use tracing::info;

use crate::{
    BaseNodeConfig, FlashblocksConfig,
    extensions::{BaseNodeExtension, ConfigurableBaseNodeExtension, FlashblocksCell, OpBuilder},
};

/// Helper struct that wires the Flashblocks canon ExEx into the node builder.
#[derive(Debug, Clone)]
pub struct FlashblocksCanonExtension {
    /// Shared Flashblocks state cache.
    pub cell: FlashblocksCell,
    /// Optional Flashblocks configuration.
    pub config: Option<FlashblocksConfig>,
}

impl FlashblocksCanonExtension {
    /// Create a new Flashblocks canon extension helper.
    pub fn new(config: &BaseNodeConfig) -> Self {
        Self { cell: config.flashblocks_cell.clone(), config: config.flashblocks.clone() }
    }
}

impl BaseNodeExtension for FlashblocksCanonExtension {
    /// Applies the extension to the supplied builder.
    fn apply(&self, builder: OpBuilder) -> OpBuilder {
        let flashblocks = self.config.clone();
        let flashblocks_enabled = flashblocks.is_some();
        let flashblocks_cell = self.cell.clone();

        builder.install_exex_if(flashblocks_enabled, "flashblocks-canon", move |mut ctx| {
            let flashblocks_cell = flashblocks_cell.clone();
            async move {
                let fb_config =
                    flashblocks.as_ref().expect("flashblocks config checked above").clone();
                let fb = flashblocks_cell
                    .get_or_init(|| {
                        Arc::new(FlashblocksState::new(
                            ctx.provider().clone(),
                            fb_config.max_pending_blocks_depth,
                        ))
                    })
                    .clone();

                Ok(async move {
                    while let Some(note) = ctx.notifications.try_next().await? {
                        let tip = match note {
                            ExExNotification::ChainCommitted { new }
                            | ExExNotification::ChainReorged { new, .. } => {
                                let tip = new.tip().num_hash();
                                let chain = Arc::unwrap_or_clone(new);
                                for (_, block) in chain.into_blocks() {
                                    fb.on_canonical_block_received(block);
                                }
                                tip
                            }
                            ExExNotification::ChainReverted { old } => old.tip().num_hash(),
                        };
                        info!(target: "flashblocks-canon", block_number = tip.number, block_hash = ?tip.hash, "Emitting FinishedHeight");
                        if let Err(e) = ctx.events.send(ExExEvent::FinishedHeight(tip)) {
                            info!(target: "flashblocks-canon", error = %e, "Failed to send FinishedHeight event");
                        }
                    }
                    Ok(())
                })
            }
        })
    }
}

impl ConfigurableBaseNodeExtension for FlashblocksCanonExtension {
    fn build(config: &BaseNodeConfig) -> eyre::Result<Self> {
        Ok(Self::new(config))
    }
}
