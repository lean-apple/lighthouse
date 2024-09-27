use crate::sync_committee_rewards::get_state_before_applying_block;
use crate::BlockId;
use crate::ExecutionOptimistic;
use beacon_chain::{BeaconChain, BeaconChainTypes};
use eth2::lighthouse::StandardBlockReward;
use std::sync::Arc;
use axum::Json;
use crate::axum_server::error::Error as AxumError;

/// The difference between block_rewards and beacon_block_rewards is the later returns block
/// reward format that satisfies beacon-api specs
pub async fn compute_beacon_block_rewards<T: BeaconChainTypes>(
    chain: Arc<BeaconChain<T>>,
    block_id: BlockId,
) -> Result<Json<(StandardBlockReward, ExecutionOptimistic, bool)>, AxumError> {
    let (block, execution_optimistic, finalized) = block_id.blinded_block(&chain)
        .map_err(|e| AxumError::BadRequest(format!("Failed to get blinded block: {:?}", e)))?;

    let block_ref = block.message();

    let block_root = block.canonical_root();

    let mut state = get_state_before_applying_block(chain.clone(), &block)
        .map_err(|e| AxumError::ServerError(format!("Failed to get state before applying block: {:?}", e)))?;

    let rewards = chain
        .compute_beacon_block_reward(block_ref, block_root, &mut state)
        .map_err(|e| AxumError::ServerError(format!("Failed to compute beacon block reward: {:?}", e)))?;

    Ok(Json((rewards, execution_optimistic, finalized)))
}
