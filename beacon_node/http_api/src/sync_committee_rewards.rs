use crate::axum_server::error::Error as AxumError;
use crate::{BlockId, ExecutionOptimistic};
use beacon_chain::{BeaconChain, BeaconChainError, BeaconChainTypes};
use eth2::lighthouse::SyncCommitteeReward;
use eth2::types::ValidatorId;
use slog::{debug, Logger};
use state_processing::BlockReplayer;
use std::sync::Arc;
use types::{BeaconState, SignedBlindedBeaconBlock};

pub fn compute_sync_committee_rewards<T: BeaconChainTypes>(
    chain: Arc<BeaconChain<T>>,
    block_id: BlockId,
    validators: Vec<ValidatorId>,
    log: Logger,
) -> Result<(Option<Vec<SyncCommitteeReward>>, ExecutionOptimistic, bool), AxumError> {
    let (block, execution_optimistic, finalized) = block_id
        .blinded_block(&chain)
        .map_err(|e| AxumError::BadRequest(format!("Failed to get blinded block: {:?}", e)))?;
    let mut state = get_state_before_applying_block(chain.clone(), &block)?;

    let reward_payload = chain
        .compute_sync_committee_rewards(block.message(), &mut state)
        .map_err(|e: BeaconChainError| {
            AxumError::ServerError(format!("Failed to compute sync committee rewards: {:?}", e))
        })?;

    let data = if reward_payload.is_empty() {
        debug!(log, "compute_sync_committee_rewards returned empty");
        None
    } else if validators.is_empty() {
        Some(reward_payload)
    } else {
        Some(
            reward_payload
                .into_iter()
                .filter(|reward| {
                    validators.iter().any(|validator| match validator {
                        ValidatorId::Index(i) => reward.validator_index == *i,
                        ValidatorId::PublicKey(pubkey) => match state.get_validator_index(pubkey) {
                            Ok(Some(i)) => reward.validator_index == i as u64,
                            _ => false,
                        },
                    })
                })
                .collect::<Vec<SyncCommitteeReward>>(),
        )
    };

    Ok((data, execution_optimistic, finalized))
}

pub fn get_state_before_applying_block<T: BeaconChainTypes>(
    chain: Arc<BeaconChain<T>>,
    block: &SignedBlindedBeaconBlock<T::EthSpec>,
) -> Result<BeaconState<T::EthSpec>, AxumError> {
    let parent_block: SignedBlindedBeaconBlock<T::EthSpec> = chain
        .get_blinded_block(&block.parent_root())
        .and_then(|maybe_block| {
            maybe_block.ok_or_else(|| BeaconChainError::MissingBeaconBlock(block.parent_root()))
        })
        .map_err(|e| AxumError::NotFound(format!("Parent block is not available! {:?}", e)))?;

    let parent_state = chain
        .get_state(&parent_block.state_root(), Some(parent_block.slot()))
        .and_then(|maybe_state| {
            maybe_state
                .ok_or_else(|| BeaconChainError::MissingBeaconState(parent_block.state_root()))
        })
        .map_err(|e| AxumError::NotFound(format!("Parent state is not available! {:?}", e)))?;

    let replayer = BlockReplayer::new(parent_state, &chain.spec)
        .no_signature_verification()
        .state_root_iter([Ok((parent_block.state_root(), parent_block.slot()))].into_iter())
        .minimal_block_root_verification()
        .apply_blocks(vec![], Some(block.slot()))
        .map_err(|e: BeaconChainError| {
            AxumError::ServerError(format!("Failed to replay block: {:?}", e))
        })?;

    Ok(replayer.into_state())
}
