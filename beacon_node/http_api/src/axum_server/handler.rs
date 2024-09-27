use super::task_spawner::{Priority, TaskSpawner};
use axum::{
    body::Body,
    extract::{Path, Query, RawQuery, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    Json,
};
use beacon_chain::attestation_verification::{Error as AttnError, VerifiedAttestation};
use beacon_chain::validator_monitor::timestamp_now;
use eth2::{
    CONSENSUS_BLOCK_VALUE_HEADER, CONSENSUS_VERSION_HEADER, CONTENT_TYPE_HEADER,
    EXECUTION_PAYLOAD_BLINDED_HEADER, EXECUTION_PAYLOAD_VALUE_HEADER, SSZ_CONTENT_TYPE_HEADER,
};
use futures::Stream;
use lighthouse_network::{NetworkGlobals, PubsubMessage};
use lighthouse_version::version_with_platform;
use network::{NetworkMessage, ValidatorSubscriptionMessage};
use slog::{debug, error, warn};
use slot_clock::SlotClock;
use ssz::Encode;
use std::convert::Infallible;
use std::{str::FromStr, sync::Arc};
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::{
    wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
    StreamExt,
};
use types::payload::BlockProductionVersion;
use types::{
    Attestation, AttestationData, ConfigAndPreset, Epoch, EthSpec, ForkName, ForkVersionedResponse,
    ProposerPreparationData, SignedAggregateAndProof, SignedBlindedBeaconBlock,
    SignedContributionAndProof, Slot, SyncCommitteeContribution, SyncCommitteeMessage,
    SyncContributionData, SyncDuty,
};

use crate::axum_server::error::Error as AxumError;
use crate::produce_block::get_randao_verification;
use crate::state_id::StateId;
use crate::validator::pubkey_to_validator_index;
use crate::version::{fork_versioned_response, inconsistent_fork_rejection};
use crate::{attester_duties, build_block_contents, proposer_duties, sync_committees, BlockId};
use crate::{
    publish_blocks, publish_network_message, publish_pubsub_message, Context, ProvenancedBlock,
};
use beacon_chain::{BeaconBlockResponseWrapper, BeaconChain, BeaconChainError, BeaconChainTypes};
use eth2::types::{
    self as api_types, BroadcastValidation, EndpointVersion, ProduceBlockV3Metadata,
    PublishBlockRequest, SyncingData, ValidatorAggregateAttestationQuery,
    ValidatorAttestationDataQuery, ValidatorBalanceData, ValidatorBalancesQuery,
    ValidatorBlocksQuery, ValidatorData, ValidatorId, ValidatorIndexData, VersionData,
};
use eth2::types::{ExecutionOptimisticFinalizedResponse, GenericResponse, GenesisData, RootData};

use super::error::Error as HandlerError;

/// Returns the `BeaconChain` otherwise returns an error
fn chain_filter<T: BeaconChainTypes>(
    ctx: &Context<T>,
) -> Result<Arc<BeaconChain<T>>, HandlerError> {
    if let Some(chain) = &ctx.chain {
        Ok(chain.clone())
    } else {
        return Err(HandlerError::Other(
            "beacon chain not available, genesis not completed".to_string(),
        ));
    }
}

// TODO(pawan): probably don't need to instantiate TaskSpawner
// repeatedly like warp
fn task_spawner<T: BeaconChainTypes>(ctx: &Context<T>) -> TaskSpawner<T::EthSpec> {
    TaskSpawner::new(
        ctx.beacon_processor_send
            .clone()
            .filter(|_| ctx.config.enable_beacon_processor),
    )
}

/// Returns the `Network` channel sender otherwise returns an error
fn network_tx<T: BeaconChainTypes>(
    ctx: &Context<T>,
) -> Result<UnboundedSender<NetworkMessage<T::EthSpec>>, HandlerError> {
    if let Some(network_tx) = &ctx.network_senders {
        Ok(network_tx.network_send())
    } else {
        return Err(HandlerError::Other(
            "The networking stack has not yet started (network_tx).".to_string(),
        ));
    }
}

/// Returns the `Network` channel sender otherwise returns an error
fn validator_subscription_tx<T: BeaconChainTypes>(
    ctx: &Context<T>,
) -> Result<Sender<ValidatorSubscriptionMessage>, HandlerError> {
    if let Some(network_tx) = &ctx.network_senders {
        Ok(network_tx.validator_subscription_send())
    } else {
        return Err(HandlerError::Other(
            "The networking stack has not yet started (network_tx).".to_string(),
        ));
    }
}

/// Returns the network globals otherwise returns an error
fn network_globals<T: BeaconChainTypes>(
    ctx: &Context<T>,
) -> Result<Arc<NetworkGlobals<T::EthSpec>>, HandlerError> {
    if let Some(globals) = &ctx.network_globals {
        Ok(globals.clone())
    } else {
        return Err(HandlerError::Other(
            "The networking stack has not yet started (network_globals).".to_string(),
        ));
    }
}

pub async fn catch_all(req: Request<axum::body::Body>) -> &'static str {
    let uri = req.uri().to_string();
    let body = req.body();
    dbg!("URI accessed: {}", uri);
    dbg!("Body accessed: {}", body);

    "woo"
}

/// GET beacon/genesis
pub async fn get_beacon_genesis<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
) -> Result<Json<GenericResponse<GenesisData>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let task_spawner = task_spawner(&ctx);
    task_spawner
        .blocking_json_task(Priority::P1, move || {
            let genesis_data = GenesisData {
                genesis_time: chain.genesis_time,
                genesis_validators_root: chain.genesis_validators_root,
                genesis_fork_version: chain.spec.genesis_fork_version,
            };
            Ok(GenericResponse::from(genesis_data))
        })
        .await
}

/// GET beacon/blocks/{block_id}/root
pub async fn get_beacon_blocks_root<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(block_id): Path<String>,
) -> Result<Json<ExecutionOptimisticFinalizedResponse<RootData>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let block_id = BlockId::from_str(&block_id)
        .map_err(|e| HandlerError::BadRequest(format!("invalid block ID: {:?}", e)))?;
    let (block_root, execution_optimistic, finalized) = block_id
        .root(&chain)
        .map_err(|e| HandlerError::ServerError(format!("failed to get block root: {:?}", e)))?;
    Ok(Json(
        api_types::GenericResponse::from(api_types::RootData::from(block_root))
            .add_execution_optimistic_finalized(execution_optimistic, finalized),
    ))
}

/// GET beacon/states/{state_id}/root
pub async fn get_beacon_state_root<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(state_id): Path<String>,
) -> Result<Json<ExecutionOptimisticFinalizedResponse<api_types::RootData>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let state_id = StateId::from_str(&state_id)
        .map_err(|e| HandlerError::BadRequest(format!("invalid state ID: {:?}", e)))?;
    let (root, execution_optimistic, finalized) = state_id
        .root(&chain)
        .map_err(|e| HandlerError::ServerError(format!("failed to get state root: {:?}", e)))?;
    Ok(Json(
        GenericResponse::from(api_types::RootData::from(root))
            .add_execution_optimistic_finalized(execution_optimistic, finalized),
    ))
}

/// GET beacon/states/{state_id}/fork
pub async fn get_beacon_state_fork<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(state_id): Path<String>,
) -> Result<Json<ExecutionOptimisticFinalizedResponse<api_types::Fork>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let state_id = StateId::from_str(&state_id)
        .map_err(|e| HandlerError::BadRequest(format!("invalid state ID: {:?}", e)))?;
    let (fork, execution_optimistic, finalized) = state_id
        .fork_and_execution_optimistic_and_finalized(&chain)
        .map_err(|e| HandlerError::ServerError(format!("failed to get state fork: {:?}", e)))?;
    Ok(Json(
        GenericResponse::from(api_types::Fork::from(fork))
            .add_execution_optimistic_finalized(execution_optimistic, finalized),
    ))
}

/// GET beacon/states/{state_id}/finality_checkpoints
pub async fn get_beacon_state_finality_checkpoints<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(state_id): Path<String>,
) -> Result<
    Json<ExecutionOptimisticFinalizedResponse<api_types::FinalityCheckpointsData>>,
    HandlerError,
> {
    let chain = chain_filter(&ctx)?;
    let state_id = StateId::from_str(&state_id)
        .map_err(|e| HandlerError::BadRequest(format!("invalid state ID: {:?}", e)))?;
    let (data, execution_optimistic, finalized) = state_id
        .map_state_and_execution_optimistic_and_finalized(
            &chain,
            |state, execution_optimistic, finalized| {
                Ok((
                    api_types::FinalityCheckpointsData {
                        previous_justified: state.previous_justified_checkpoint(),
                        current_justified: state.current_justified_checkpoint(),
                        finalized: state.finalized_checkpoint(),
                    },
                    execution_optimistic,
                    finalized,
                ))
            },
        )
        .map_err(|e| {
            HandlerError::ServerError(format!("failed to get finality checkpoints: {:?}", e))
        })?;
    Ok(Json(ExecutionOptimisticFinalizedResponse {
        data,
        execution_optimistic: Some(execution_optimistic),
        finalized: Some(finalized),
    }))
}

/// Get sse events
pub async fn get_events<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    RawQuery(query): RawQuery, // Should probably have a cleaner solution for this
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let topics = if let Some(query_str) = query {
        dbg!(&query_str);
        let event_query: api_types::EventQuery =
            serde_array_query::from_str(&query_str).map_err(|e| {
                HandlerError::BadRequest(format!(
                    "Failed to parse query string: Query string: {} error: {:?}",
                    query_str, e
                ))
            })?;
        event_query.topics
    } else {
        vec![]
    };
    // for each topic subscribed spawn a new subscription
    let mut receivers = Vec::with_capacity(topics.len());
    if let Some(event_handler) = chain.event_handler.as_ref() {
        for topic in topics {
            let receiver = match topic {
                api_types::EventTopic::Head => event_handler.subscribe_head(),
                api_types::EventTopic::Block => event_handler.subscribe_block(),
                api_types::EventTopic::BlobSidecar => event_handler.subscribe_blob_sidecar(),
                api_types::EventTopic::Attestation => event_handler.subscribe_attestation(),
                api_types::EventTopic::VoluntaryExit => event_handler.subscribe_exit(),
                api_types::EventTopic::FinalizedCheckpoint => event_handler.subscribe_finalized(),
                api_types::EventTopic::ChainReorg => event_handler.subscribe_reorgs(),
                api_types::EventTopic::ContributionAndProof => {
                    event_handler.subscribe_contributions()
                }
                api_types::EventTopic::PayloadAttributes => {
                    event_handler.subscribe_payload_attributes()
                }
                api_types::EventTopic::LateHead => event_handler.subscribe_late_head(),
                api_types::EventTopic::LightClientFinalityUpdate => {
                    event_handler.subscribe_light_client_finality_update()
                }
                api_types::EventTopic::LightClientOptimisticUpdate => {
                    event_handler.subscribe_light_client_optimistic_update()
                }
                api_types::EventTopic::BlockReward => event_handler.subscribe_block_reward(),
                api_types::EventTopic::AttesterSlashing => {
                    event_handler.subscribe_attester_slashing()
                }
                api_types::EventTopic::BlsToExecutionChange => {
                    event_handler.subscribe_bls_to_execution_change()
                }
                api_types::EventTopic::ProposerSlashing => {
                    event_handler.subscribe_proposer_slashing()
                }
            };

            receivers.push(
                BroadcastStream::new(receiver)
                    .map(|msg| {
                        match msg {
                            Ok(data) => Event::default()
                                .event(data.topic_name())
                                .json_data(data)
                                .unwrap_or_else(|e| {
                                    Event::default().comment(format!("error - bad json: {e:?}"))
                                }),
                            // Do not terminate the stream if the channel fills
                            // up. Just drop some messages and send a comment to
                            // the client.
                            Err(BroadcastStreamRecvError::Lagged(n)) => {
                                Event::default().comment(format!("error - dropped {n} messages"))
                            }
                        }
                    })
                    .map(Ok::<_, std::convert::Infallible>),
            );
        }
    } else {
        return Err(HandlerError::ServerError(
            "event handler was not initialized".to_string(),
        ));
    }

    let s = futures::stream::select_all(receivers);
    Ok(Sse::new(s).keep_alive(axum::response::sse::KeepAlive::new()))
}

/// GET beacon/states/{state_id}/validator_balances?id
pub async fn get_beacon_state_validator_balances<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(state_id): Path<String>,
    RawQuery(query): RawQuery, // Should probably have a cleaner solution for this
) -> Result<Json<ExecutionOptimisticFinalizedResponse<Vec<ValidatorBalanceData>>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let state_id = StateId::from_str(&state_id)
        .map_err(|e| HandlerError::BadRequest(format!("invalid state ID: {:?}", e)))?;

    let validator_queries = if let Some(query_str) = query {
        let validator_queries: ValidatorBalancesQuery = serde_array_query::from_str(&query_str)
            .map_err(|e| {
                HandlerError::BadRequest(format!(
                    "failed to parse query string: Query string: {} error: {:?}",
                    query_str, e
                ))
            })?;
        validator_queries.id
    } else {
        None
    };

    let response = crate::validators::get_beacon_state_validator_balances(
        state_id,
        chain,
        validator_queries.as_deref(),
    )
    .await
    .map_err(|e| HandlerError::ServerError(format!("failed to get validator balances: {:?}", e)))?;

    Ok(Json(response))
}

/// GET beacon/states/{state_id}/validators/{validator_id}
pub async fn get_beacon_state_validators_id<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path((state_id, validator_id)): Path<(String, ValidatorId)>,
) -> Result<Json<ExecutionOptimisticFinalizedResponse<ValidatorData>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let state_id = StateId::from_str(&state_id)
        .map_err(|e| HandlerError::BadRequest(format!("invalid state ID: {:?}", e)))?;

    let (data, execution_optimistic, finalized) = state_id
        .map_state_and_execution_optimistic_and_finalized(
            &chain,
            |state, execution_optimistic, finalized| {
                let index_opt = match &validator_id {
                    ValidatorId::PublicKey(pubkey) => {
                        pubkey_to_validator_index(&chain, state, pubkey).map_err(|e| {
                            HandlerError::NotFound(format!(
                                "unable to access pubkey cache: {:?}",
                                e
                            ))
                        })?
                    }
                    ValidatorId::Index(index) => Some(*index as usize),
                };

                let validator_data = index_opt
                    .and_then(|index| {
                        let validator = state.validators().get(index)?;
                        let balance = *state.balances().get(index)?;
                        let epoch = state.current_epoch();
                        let far_future_epoch = chain.spec.far_future_epoch;

                        Some(api_types::ValidatorData {
                            index: index as u64,
                            balance,
                            status: api_types::ValidatorStatus::from_validator(
                                validator,
                                epoch,
                                far_future_epoch,
                            ),
                            validator: validator.clone(),
                        })
                    })
                    .ok_or_else(|| {
                        HandlerError::NotFound(format!("unknown validator: {}", validator_id))
                    })?;
                Ok((validator_data, execution_optimistic, finalized))
            },
        )
        .map_err(|e| HandlerError::ServerError(format!("failed to get validator data: {:?}", e)))?;

    Ok(Json(api_types::ExecutionOptimisticFinalizedResponse {
        data,
        execution_optimistic: Some(execution_optimistic),
        finalized: Some(finalized),
    }))
}

/// TODO: investigate merging ssz and json handlers
/// beacon/blinded_blocks
pub async fn post_beacon_blinded_blocks_json<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    _header_map: HeaderMap,
    Json(block_contents): Json<Arc<SignedBlindedBeaconBlock<T::EthSpec>>>,
) -> Result<Response, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();
    let _warp_response = publish_blocks::publish_blinded_block(
        block_contents,
        chain,
        &network_tx,
        log,
        BroadcastValidation::default(),
        ctx.config.duplicate_block_status_code,
    )
    .await?;
    Ok(Response::new(().into()))
}

/// v2/beacon/blinded_blocks
pub async fn post_beacon_blinded_blocks_json_v2<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    _header_map: HeaderMap,
    Query(validation_level): Query<api_types::BroadcastValidationQuery>,
    Json(block_contents): Json<Arc<SignedBlindedBeaconBlock<T::EthSpec>>>,
) -> Result<Response, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();
    let _warp_response = publish_blocks::publish_blinded_block(
        block_contents,
        chain,
        &network_tx,
        log,
        validation_level.broadcast_validation,
        ctx.config.duplicate_block_status_code,
    )
    .await?;
    Ok(Response::new(().into()))
}

/// TODO: investigate merging ssz and json handlers
/// beacon/blinded_blocks
pub async fn post_beacon_blocks_json<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    _header_map: HeaderMap,
    Json(block_contents): Json<PublishBlockRequest<T::EthSpec>>,
) -> Result<Response, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();
    let _warp_response = publish_blocks::publish_block(
        None,
        ProvenancedBlock::local(block_contents),
        chain,
        &network_tx,
        log,
        BroadcastValidation::default(),
        ctx.config.duplicate_block_status_code,
    )
    .await?;
    Ok(Response::new(().into()))
}

/// TODO: investigate merging ssz and json handlers
/// beacon/blinded_blocks
pub async fn post_beacon_blocks_json_v2<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    _header_map: HeaderMap,
    Query(validation_level): Query<api_types::BroadcastValidationQuery>,
    Json(block_contents): Json<PublishBlockRequest<T::EthSpec>>,
) -> Result<Response, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();
    let _warp_response = publish_blocks::publish_block(
        None,
        ProvenancedBlock::local(block_contents),
        chain,
        &network_tx,
        log,
        validation_level.broadcast_validation,
        ctx.config.duplicate_block_status_code,
    )
    .await?;
    Ok(Response::new(().into()))
}

/// POST beacon/pool/attestations
pub async fn post_beacon_pool_attestations<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    _header_map: HeaderMap,
    Json(attestations): Json<Vec<Attestation<T::EthSpec>>>,
) -> Result<(), HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();

    let seen_timestamp = timestamp_now();
    let mut failures = Vec::new();
    let mut num_already_known = 0;

    for (index, attestation) in attestations.as_slice().iter().enumerate() {
        let attestation = match chain.verify_unaggregated_attestation_for_gossip(attestation, None)
        {
            Ok(attestation) => attestation,
            Err(AttnError::PriorAttestationKnown { .. }) => {
                num_already_known += 1;

                // Skip to the next attestation since an attestation for this
                // validator is already known in this epoch.
                //
                // There's little value for the network in validating a second
                // attestation for another validator since it is either:
                //
                // 1. A duplicate.
                // 2. Slashable.
                // 3. Invalid.
                //
                // We are likely to get duplicates in the case where a VC is using
                // fallback BNs. If the first BN actually publishes some/all of a
                // batch of attestations but fails to respond in a timely fashion,
                // the VC is likely to try publishing the attestations on another
                // BN. That second BN may have already seen the attestations from
                // the first BN and therefore indicate that the attestations are
                // "already seen". An attestation that has already been seen has
                // been published on the network so there's no actual error from
                // the perspective of the user.
                //
                // It's better to prevent slashable attestations from ever
                // appearing on the network than trying to slash validators,
                // especially those validators connected to the local API.
                //
                // There might be *some* value in determining that this attestation
                // is invalid, but since a valid attestation already it exists it
                // appears that this validator is capable of producing valid
                // attestations and there's no immediate cause for concern.
                continue;
            }
            Err(e) => {
                error!(log,
                    "Failure verifying attestation for gossip";
                    "error" => ?e,
                    "request_index" => index,
                    "committee_index" => attestation.data().index,
                    "attestation_slot" => attestation.data().slot,
                );
                failures.push(api_types::Failure::new(
                    index,
                    format!("Verification: {:?}", e),
                ));
                // skip to the next attestation so we do not publish this one to gossip
                continue;
            }
        };

        // Notify the validator monitor.
        chain
            .validator_monitor
            .read()
            .register_api_unaggregated_attestation(
                seen_timestamp,
                attestation.indexed_attestation(),
                &chain.slot_clock,
            );

        publish_pubsub_message(
            &network_tx,
            PubsubMessage::Attestation(Box::new((
                attestation.subnet_id(),
                attestation.attestation().clone_as_attestation(),
            ))),
        )?;

        let committee_index = attestation.attestation().data().index;
        let slot = attestation.attestation().data().slot;

        if let Err(e) = chain.apply_attestation_to_fork_choice(&attestation) {
            error!(log,
                "Failure applying verified attestation to fork choice";
                "error" => ?e,
                "request_index" => index,
                "committee_index" => committee_index,
                "slot" => slot,
            );
            failures.push(api_types::Failure::new(
                index,
                format!("Fork choice: {:?}", e),
            ));
        };

        if let Err(e) = chain.add_to_naive_aggregation_pool(&attestation) {
            error!(log,
                "Failure adding verified attestation to the naive aggregation pool";
                "error" => ?e,
                "request_index" => index,
                "committee_index" => committee_index,
                "slot" => slot,
            );
            failures.push(api_types::Failure::new(
                index,
                format!("Naive aggregation pool: {:?}", e),
            ));
        }
    }

    if num_already_known > 0 {
        debug!(
            log,
            "Some unagg attestations already known";
            "count" => num_already_known
        );
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(HandlerError::BadRequest(format!(
            "error processing attestations: {:?}",
            failures
        )))
    }
}

/// POST beacon/pool/sync_committees
pub async fn post_beacon_pool_sync_committees<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    _header_map: HeaderMap,
    Json(signatures): Json<Vec<SyncCommitteeMessage>>,
) -> Result<(), HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();

    sync_committees::process_sync_committee_signatures(signatures, network_tx, &chain, log)?;
    Ok(())
}

/// GET node/syncing
pub async fn get_node_syncing<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
) -> Result<Json<GenericResponse<SyncingData>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_globals = network_globals(&ctx)?;

    let el_offline = if let Some(el) = &chain.execution_layer {
        el.is_offline_or_erroring().await
    } else {
        true
    };

    let head_slot = chain.canonical_head.cached_head().head_slot();
    let current_slot = chain
        .slot_clock
        .now_or_genesis()
        .ok_or_else(|| HandlerError::ServerError("Unable to read slot clock".to_string()))?;

    // Taking advantage of saturating subtraction on slot.
    let sync_distance = current_slot - head_slot;

    let is_optimistic = chain
        .is_optimistic_or_invalid_head()
        .map_err(|e| HandlerError::BeaconChainError(format!("Beacon chain error: {:?}", e)))?;

    let syncing_data = SyncingData {
        is_syncing: network_globals.sync_state.read().is_syncing(),
        is_optimistic: Some(is_optimistic),
        el_offline: Some(el_offline),
        head_slot,
        sync_distance,
    };

    Ok(Json(GenericResponse::from(syncing_data)))
}

/// GET config/spec
pub async fn get_node_version() -> Result<Json<GenericResponse<VersionData>>, HandlerError> {
    Ok(api_types::GenericResponse::from(VersionData {
        version: version_with_platform(),
    }))
    .map(Json)
}

/// GET node/syncing
pub async fn get_config_spec<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
) -> Result<Json<GenericResponse<ConfigAndPreset>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let spec_fork_name = ctx.config.spec_fork_name;
    let config_and_preset =
        ConfigAndPreset::from_chain_spec::<T::EthSpec>(&chain.spec, spec_fork_name);
    Ok(api_types::GenericResponse::from(config_and_preset)).map(Json)
}

/// POST validator/duties/attester/{epoch}
pub async fn post_validator_duties_attester<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(epoch): Path<Epoch>,
    Json(indices): Json<ValidatorIndexData>,
) -> Result<Json<api_types::DutiesResponse<Vec<api_types::AttesterData>>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    attester_duties::attester_duties(epoch, &indices.0, &chain)
        .map_err(|e| HandlerError::Other(format!("Attester duties error: {:?}", e)))
        .map(Json)
}
/// GET validator/duties/proposer/{epoch}
pub async fn get_validator_duties_proposer<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(epoch): Path<Epoch>,
) -> Result<Json<api_types::DutiesResponse<Vec<api_types::ProposerData>>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let log = ctx.log.clone();
    proposer_duties::proposer_duties(epoch, &chain, &log)
        .map_err(|e| HandlerError::Other(format!("Proposer duties error: {:?}", e)))
        .map(Json)
}

/// POST validator/duties/sync/{epoch}
pub async fn post_validator_duties_sync<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(epoch): Path<Epoch>,
    Json(indices): Json<ValidatorIndexData>,
) -> Result<Json<api_types::ExecutionOptimisticResponse<Vec<SyncDuty>>>, AxumError> {
    let chain = chain_filter(&ctx)?;
    sync_committees::sync_committee_duties(epoch, &indices.0, &chain)
}

async fn produce_block<T: BeaconChainTypes>(
    chain: Arc<BeaconChain<T>>,
    slot: Slot,
    query: api_types::ValidatorBlocksQuery,
    version: BlockProductionVersion,
) -> Result<(BeaconBlockResponseWrapper<T::EthSpec>, ForkName), HandlerError> {
    let randao_reveal = query.randao_reveal.decompress().map_err(|e| {
        HandlerError::InvalidRandaoReveal(format!(
            "RANDAO reveal is not a valid BLS signature: {:?}",
            e
        ))
    })?;

    let randao_verification = get_randao_verification(&query, randao_reveal.is_infinity())
        .map_err(|e| HandlerError::BadRequest(format!("Invalid randao verification: {:?}", e)))?;

    let block_response = chain
        .produce_block_with_verification(
            randao_reveal,
            slot,
            query.graffiti.map(Into::into),
            randao_verification,
            None,
            version,
        )
        .await
        .map_err(|e| HandlerError::BlockProductionError(format!("{:?}", e)))?;

    let fork_name = block_response
        .fork_name(&chain.spec)
        .map_err(inconsistent_fork_rejection)?;

    Ok((block_response, fork_name))
}

/// GET v2/validator/blocks/{slot}
pub async fn get_validator_blocks_v2<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(slot): Path<Slot>,
    header_map: HeaderMap,
    Query(query): Query<ValidatorBlocksQuery>,
) -> Result<impl IntoResponse, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let accept_header = header_map
        .get("accept")
        .and_then(|val| val.to_str().ok())
        .and_then(|val| api_types::Accept::from_str(val).ok());

    let (block_response, fork_name) =
        produce_block(chain, slot, query, BlockProductionVersion::FullV2).await?;

    let block_contents = build_block_contents::build_block_contents(fork_name, block_response)
        .map_err(|e| HandlerError::Other(format!("failed to build block contents: {:?}", e)))?;

    match accept_header {
        Some(api_types::Accept::Ssz) => {
            let ssz_bytes = block_contents.as_ssz_bytes();
            let mut headers = HeaderMap::new();
            headers.insert(
                CONTENT_TYPE_HEADER,
                HeaderValue::from_static(SSZ_CONTENT_TYPE_HEADER),
            );
            headers.insert(
                CONSENSUS_VERSION_HEADER,
                HeaderValue::from_str(&fork_name.to_string())
                    .map_err(|_| HandlerError::ServerError("invalid consensus version header value".to_string()))?,
            );

            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE_HEADER, SSZ_CONTENT_TYPE_HEADER)
                .header(CONSENSUS_VERSION_HEADER, fork_name.to_string())
                .body(Body::from(ssz_bytes))
                .map_err(|e| {
                    HandlerError::ServerError(format!("failed to create SSZ response: {}", e))
                })?)
        }
        _ => {
            let json_response = fork_versioned_response(
                EndpointVersion(2),
                fork_name,
                block_contents,
            )
            .map_err(|e| {
                HandlerError::ServerError(format!("failed to create JSON response: {:?}", e))
            })?;

            let mut response = Json(json_response).into_response();
            response.headers_mut().insert(
                CONSENSUS_VERSION_HEADER,
                HeaderValue::from_str(&fork_name.to_string())
                    .map_err(|_| HandlerError::ServerError("invalid header value".to_string()))?,
            );
            Ok(response)
        }
    }
}

/// GET v3/validator/blocks/{slot}
pub async fn get_validator_blocks_v3<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Path(slot): Path<Slot>,
    header_map: HeaderMap,
    Query(query): Query<ValidatorBlocksQuery>,
) -> Result<impl IntoResponse, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let accept_header = header_map
        .get("accept")
        .and_then(|val| val.to_str().ok())
        .and_then(|val| api_types::Accept::from_str(val).ok());

    let (block_response, fork_name) =
        produce_block(chain, slot, query, BlockProductionVersion::FullV2).await?;

    let execution_payload_value = block_response.execution_payload_value();
    let consensus_block_value = block_response.consensus_block_value_wei();
    let execution_payload_blinded = block_response.is_blinded();

    let metadata = ProduceBlockV3Metadata {
        consensus_version: fork_name,
        execution_payload_blinded,
        execution_payload_value,
        consensus_block_value,
    };

    let block_contents = build_block_contents::build_block_contents(fork_name, block_response)
        .map_err(|e| HandlerError::Other(format!("failed to build block contents: {:?}", e)))?;

    match accept_header {
        Some(api_types::Accept::Ssz) => {
            let body = block_contents.as_ssz_bytes();
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE_HEADER, SSZ_CONTENT_TYPE_HEADER)
                .header(CONSENSUS_VERSION_HEADER, fork_name.to_string())
                .header(
                    EXECUTION_PAYLOAD_BLINDED_HEADER,
                    execution_payload_blinded.to_string(),
                )
                .header(
                    EXECUTION_PAYLOAD_VALUE_HEADER,
                    execution_payload_value.to_string(),
                )
                .header(
                    CONSENSUS_BLOCK_VALUE_HEADER,
                    consensus_block_value.to_string(),
                )
                .body(Body::from(body))
                .map_err(|e| {
                    HandlerError::ServerError(format!("failed to create SSZ response: {}", e))
                })?)
        }
        _ => {
            let json_response = ForkVersionedResponse {
                version: Some(fork_name),
                metadata,
                data: block_contents,
            };

            let mut response = Json(json_response).into_response();
            let headers = response.headers_mut();
            headers.insert(
                CONSENSUS_VERSION_HEADER,
                HeaderValue::from_str(&fork_name.to_string()).map_err(|_| {
                    HandlerError::Other("Invalid consensus version header value".to_string())
                })?,
            );
            headers.insert(
                EXECUTION_PAYLOAD_BLINDED_HEADER,
                HeaderValue::from_str(&execution_payload_blinded.to_string()).map_err(|_| {
                    HandlerError::Other(
                        "Invalid execution payload blinder header value".to_string(),
                    )
                })?,
            );
            headers.insert(
                EXECUTION_PAYLOAD_VALUE_HEADER,
                HeaderValue::from_str(&execution_payload_value.to_string()).map_err(|_| {
                    HandlerError::Other("Invalid execution payload header value".to_string())
                })?,
            );
            headers.insert(
                CONSENSUS_BLOCK_VALUE_HEADER,
                HeaderValue::from_str(&consensus_block_value.to_string()).map_err(|_| {
                    HandlerError::Other("Invalid consensus block header value".to_string())
                })?,
            );
            Ok(response)
        }
    }
}

/// GET validator/attestation_data?slot,committee_index
pub async fn get_validator_attestation_data<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Query(query): Query<ValidatorAttestationDataQuery>,
) -> Result<Json<GenericResponse<AttestationData>>, HandlerError> {
    let chain = chain_filter(&ctx)?;
    let current_slot = chain.slot().map_err(|e| {
        HandlerError::BeaconChainError(format!("Failed to get current slot: {:?}", e))
    })?;

    // allow a tolerance of one slot to account for clock skew
    if query.slot > current_slot + 1 {
        return Err(HandlerError::BadRequest(format!(
            "Request slot {} is more than one slot past the current slot {}",
            query.slot, current_slot
        )));
    }

    let attestation_data = chain
        .produce_unaggregated_attestation(query.slot, query.committee_index)
        .map_err(|e| {
            HandlerError::BeaconChainError(format!("Failed to produce attestation: {:?}", e))
        })?
        .data()
        .clone();

    Ok(Json(api_types::GenericResponse::from(attestation_data)))
}

/// GET validator/aggregate_attestation?slot,committee_index
pub async fn get_validator_aggregate_attestation<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Query(query): Query<ValidatorAggregateAttestationQuery>,
) -> Result<Json<GenericResponse<Attestation<T::EthSpec>>>, HandlerError> {
    let chain = chain_filter(&ctx)?;

    let aggregate = chain
        .get_pre_electra_aggregated_attestation_by_slot_and_root(
            query.slot,
            &query.attestation_data_root,
        )
        .map_err(|e: BeaconChainError| {
            HandlerError::BadRequest(format!("unable to fetch aggregate: {:?}", e))
        })?
        .ok_or_else(|| HandlerError::NotFound("no matching aggregate found".to_string()))?;

    Ok(Json(GenericResponse::from(aggregate)))
}

/// POST validator/aggregate_and_proofs
pub async fn post_validator_aggregate_and_proofs<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Json(aggregates): Json<Vec<SignedAggregateAndProof<T::EthSpec>>>,
) -> Result<(), HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();

    let seen_timestamp = timestamp_now();
    let mut verified_aggregates = Vec::with_capacity(aggregates.len());
    let mut messages = Vec::with_capacity(aggregates.len());
    let mut failures = Vec::new();

    // Verify that all messages in the post are valid before processing further
    for (index, aggregate) in aggregates.iter().enumerate() {
        match chain.verify_aggregated_attestation_for_gossip(aggregate) {
            Ok(verified_aggregate) => {
                messages.push(PubsubMessage::AggregateAndProofAttestation(Box::new(
                    verified_aggregate.aggregate().clone(),
                )));

                // Notify the validator monitor.
                chain
                    .validator_monitor
                    .read()
                    .register_api_aggregated_attestation(
                        seen_timestamp,
                        verified_aggregate.aggregate(),
                        verified_aggregate.indexed_attestation(),
                        &chain.slot_clock,
                    );

                verified_aggregates.push((index, verified_aggregate));
            }
            // If we already know the attestation, don't broadcast it or attempt to
            // further verify it. Return success.
            //
            // It's reasonably likely that two different validators produce
            // identical aggregates, especially if they're using the same beacon
            // node.
            Err(AttnError::AttestationSupersetKnown(_)) => continue,
            // If we've already seen this aggregator produce an aggregate, just
            // skip this one.
            //
            // We're likely to see this with VCs that use fallback BNs. The first
            // BN might time-out *after* publishing the aggregate and then the
            // second BN will indicate it's already seen the aggregate.
            //
            // There's no actual error for the user or the network since the
            // aggregate has been successfully published by some other node.
            Err(AttnError::AggregatorAlreadyKnown(_)) => continue,
            Err(e) => {
                error!(log,
                    "Failure verifying aggregate and proofs";
                    "error" => format!("{:?}", e),
                    "request_index" => index,
                    "aggregator_index" => aggregate.message().aggregator_index(),
                    "attestation_index" => aggregate.message().aggregate().data().index,
                    "attestation_slot" => aggregate.message().aggregate().data().slot,
                );
                failures.push(api_types::Failure::new(
                    index,
                    format!("Verification: {:?}", e),
                ));
            }
        }
    }

    // Publish aggregate attestations to the libp2p network
    if !messages.is_empty() {
        publish_network_message(&network_tx, NetworkMessage::Publish { messages }).map_err(
            |e| HandlerError::Other(format!("failed to publish network message: {:?}", e)),
        )?;
    }

    // Import aggregate attestations
    for (index, verified_aggregate) in verified_aggregates {
        if let Err(e) = chain.apply_attestation_to_fork_choice(&verified_aggregate) {
            error!(log,
                "Failure applying verified aggregate attestation to fork choice";
                "error" => format!("{:?}", e),
                "request_index" => index,
                "aggregator_index" => verified_aggregate.aggregate().message().aggregator_index(),
                "attestation_index" => verified_aggregate.attestation().data().index,
                "attestation_slot" => verified_aggregate.attestation().data().slot,
            );
            failures.push(api_types::Failure::new(
                index,
                format!("Fork choice: {:?}", e),
            ));
        }
        if let Err(e) = chain.add_to_block_inclusion_pool(verified_aggregate) {
            warn!(
                log,
                "Could not add verified aggregate attestation to the inclusion pool";
                "error" => ?e,
                "request_index" => index,
            );
            failures.push(api_types::Failure::new(index, format!("Op pool: {:?}", e)));
        }
    }

    if !failures.is_empty() {
        Err(HandlerError::BadRequest(format!(
            "error processing aggregate and proofs: {:?}",
            failures
        )))
    } else {
        Ok(())
    }
}

/// POST validator/beacon_committee_subscriptions
pub async fn post_validator_beacon_committee_subscriptions<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Json(subscriptions): Json<Vec<api_types::BeaconCommitteeSubscription>>,
) -> Result<(), HandlerError> {
    let chain = chain_filter(&ctx)?;
    let log = ctx.log.clone();
    let validator_subscription_tx = validator_subscription_tx(&ctx)?;

    let subscriptions: std::collections::BTreeSet<_> = subscriptions
        .iter()
        .map(|subscription| {
            chain
                .validator_monitor
                .write()
                .auto_register_local_validator(subscription.validator_index);
            api_types::ValidatorSubscription {
                attestation_committee_index: subscription.committee_index,
                slot: subscription.slot,
                committee_count_at_slot: subscription.committees_at_slot,
                is_aggregator: subscription.is_aggregator,
            }
        })
        .collect();

    let message = ValidatorSubscriptionMessage::AttestationSubscribe { subscriptions };
    if let Err(e) = validator_subscription_tx.try_send(message) {
        warn!(
            log,
            "Unable to process committee subscriptions";
            "info" => "the host may be overloaded or resource-constrained",
            "error" => ?e,
        );
        return Err(HandlerError::ServerError(
            "unable to queue subscription, host may be overloaded or shutting down".to_string(),
        ));
    }

    Ok(())
}

/// POST validator/sync_committee_subscriptions
pub async fn post_validator_sync_committee_subscriptions<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Json(subscriptions): Json<Vec<types::SyncCommitteeSubscription>>,
) -> Result<(), HandlerError> {
    let chain = chain_filter(&ctx)?;
    let log = ctx.log.clone();
    let validator_subscription_tx = validator_subscription_tx(&ctx)?;

    for subscription in subscriptions {
        chain
            .validator_monitor
            .write()
            .auto_register_local_validator(subscription.validator_index);

        let message = ValidatorSubscriptionMessage::SyncCommitteeSubscribe {
            subscriptions: vec![subscription],
        };
        if let Err(e) = validator_subscription_tx.try_send(message) {
            warn!(
                log,
                "Unable to process sync subscriptions";
                "info" => "the host may be overloaded or resource-constrained",
                "error" => ?e
            );
            return Err(HandlerError::ServerError(
                "unable to queue subscription, host may be overloaded or shutting down".to_string(),
            ));
        }
    }

    Ok(())
}

/// GET validator/sync_committee_contribution
pub async fn get_validator_sync_committee_contribution<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Query(sync_committee_data): Query<SyncContributionData>,
) -> Result<Json<GenericResponse<SyncCommitteeContribution<T::EthSpec>>>, HandlerError> {
    let chain = chain_filter(&ctx)?;

    chain
        .get_aggregated_sync_committee_contribution(&sync_committee_data)
        .map_err(|e| {
            HandlerError::BadRequest(format!("Unable to fetch sync contribution: {:?}", e))
        })?
        .map(|contribution| Json(GenericResponse::from(contribution)))
        .ok_or_else(|| HandlerError::NotFound("No matching sync contribution found".to_string()))
}

/// GET validator/contribution_and_proofs
pub async fn post_validator_contribution_and_proofs<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Json(contributions): Json<Vec<SignedContributionAndProof<T::EthSpec>>>,
) -> Result<(), HandlerError> {
    let chain = chain_filter(&ctx)?;
    let network_tx = network_tx(&ctx)?;
    let log = ctx.log.clone();

    sync_committees::process_signed_contribution_and_proofs(
        contributions,
        network_tx,
        &chain,
        log,
    )?;
    Ok(())
}

/// POST validator/prepare_beacon_proposer
pub async fn post_validator_prepare_beacon_proposer<T: BeaconChainTypes>(
    State(ctx): State<Arc<Context<T>>>,
    Json(preparation_data): Json<Vec<ProposerPreparationData>>,
) -> Result<(), HandlerError> {
    let chain = chain_filter(&ctx)?;
    let log = ctx.log.clone();

    // TODO: Improve BeaconChainError specification
    let execution_layer = chain
        .execution_layer
        .as_ref()
        .ok_or(HandlerError::BeaconChainError(
            "Execution layer missing".to_string(),
        ))?;

    let current_slot = chain.slot().map_err(|e| {
        HandlerError::BeaconChainError(format!("Unable to get current slot: {:?}", e))
    })?;
    let current_epoch = current_slot.epoch(T::EthSpec::slots_per_epoch());

    debug!(
        log,
        "Received proposer preparation data";
        "count" => preparation_data.len(),
    );

    execution_layer
        .update_proposer_preparation(current_epoch, &preparation_data)
        .await;

    chain
        .prepare_beacon_proposer(current_slot)
        .await
        .map_err(|e| {
            HandlerError::BadRequest(format!("Error updating proposer preparations: {:?}", e))
        })?;

    Ok(())
}
