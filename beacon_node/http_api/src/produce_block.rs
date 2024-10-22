use crate::{
    build_block_contents,
    version::{
        add_consensus_block_value_header, add_consensus_version_header,
        add_execution_payload_blinded_header, add_execution_payload_value_header,
        add_ssz_content_type_header, fork_versioned_response,
    },
};
use beacon_chain::{
    BeaconBlockResponseWrapper, BeaconChain, BeaconChainTypes, ProduceBlockVerification,
};
use eth2::types::{
    self as api_types, EndpointVersion, ProduceBlockV3Metadata, SkipRandaoVerification,
};
use ssz::Encode;
use std::sync::Arc;
use types::{payload::BlockProductionVersion, *};
use axum::{
    body::Body,
    extract::{Query, State, Path},
    response::{IntoResponse, Response},
    http::{StatusCode, header::CONTENT_TYPE},
};
use crate::axum_server::error::Error as AxumError;

/// If default boost factor is provided in validator/blocks v3 request, we will skip the calculation
/// to keep the precision.
const DEFAULT_BOOST_FACTOR: u64 = 100;

pub fn get_randao_verification(
    query: &api_types::ValidatorBlocksQuery,
    randao_reveal_infinity: bool,
) -> Result<ProduceBlockVerification, AxumError> {
    let randao_verification = if query.skip_randao_verification == SkipRandaoVerification::Yes {
        if !randao_reveal_infinity {
            return Err(AxumError::BadRequest(
                "randao_reveal must be point-at-infinity if verification is skipped".into(),
            ));
        }
        ProduceBlockVerification::NoVerification
    } else {
        ProduceBlockVerification::VerifyRandao
    };

    Ok(randao_verification)
}

pub async fn produce_block_v3<T: BeaconChainTypes>(
    State(chain): State<Arc<BeaconChain<T>>>,
    Path(slot): Path<Slot>,
    Query(query): Query<api_types::ValidatorBlocksQuery>,
    accept_header: Option<api_types::Accept>,
) -> Result<impl IntoResponse, AxumError> {
    let randao_reveal = query.randao_reveal.decompress().map_err(|e| {
        AxumError::BadRequest(format!(
            "randao reveal is not a valid BLS signature: {:?}",
            e
        ))
    })?;

    let randao_verification = get_randao_verification(&query, randao_reveal.is_infinity())?;
    let builder_boost_factor = if query.builder_boost_factor == Some(DEFAULT_BOOST_FACTOR) {
        None
    } else {
        query.builder_boost_factor
    };

    let block_response_type = chain
        .produce_block_with_verification(
            randao_reveal,
            slot,
            query.graffiti,
            randao_verification,
            builder_boost_factor,
            BlockProductionVersion::V3,
        )
        .await
        .map_err(|e| AxumError::BadRequest(format!("failed to fetch a block: {:?}", e)))?;

    build_response_v3(chain, block_response_type, accept_header)
}

pub fn build_response_v3<T: BeaconChainTypes>(
    chain: Arc<BeaconChain<T>>,
    block_response: BeaconBlockResponseWrapper<T::EthSpec>,
    accept_header: Option<api_types::Accept>,
) -> Result<Response<Body>, AxumError> {
    let fork_name = block_response
        .fork_name(&chain.spec)
        .map_err(|e| AxumError::ServerError(format!("Inconsistent fork: {:?}", e)))?;
    let execution_payload_value = block_response.execution_payload_value();
    let consensus_block_value = block_response.consensus_block_value_wei();
    let execution_payload_blinded = block_response.is_blinded();

    let metadata = ProduceBlockV3Metadata {
        consensus_version: fork_name,
        execution_payload_blinded,
        execution_payload_value,
        consensus_block_value,
    };

    let block_contents = build_block_contents::build_block_contents(fork_name, block_response)?;

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .unwrap();

    response = add_consensus_version_header(response, fork_name)?;
    response = add_execution_payload_blinded_header(response, execution_payload_blinded)?;
    response = add_execution_payload_value_header(response, execution_payload_value)?;
    response = add_consensus_block_value_header(response, consensus_block_value)?;

    match accept_header {
        Some(api_types::Accept::Ssz) => {
            response = add_ssz_content_type_header(response);
            *response.body_mut() = Body::from(block_contents.as_ssz_bytes());
            Ok(response)
        },
        _ => {
            response.headers_mut().insert(CONTENT_TYPE, "application/json".parse().unwrap());
            let json_response = fork_versioned_response(EndpointVersion::V3, fork_name, block_contents)?;
            *response.body_mut() = Body::from(serde_json::to_vec(&json_response)?);
            Ok(response)
        }
    }
}

pub async fn produce_blinded_block_v2<T: BeaconChainTypes>(
    State(chain): State<Arc<BeaconChain<T>>>,
    Query(query): Query<api_types::ValidatorBlocksQuery>,
    Path(slot): Path<Slot>,
    endpoint_version: EndpointVersion,
    accept_header: Option<api_types::Accept>,
) -> Result<impl IntoResponse, AxumError> {
    let randao_reveal = query.randao_reveal.decompress().map_err(|e| {
        AxumError::BadRequest(format!(
            "randao reveal is not a valid BLS signature: {:?}",
            e
        ))
    })?;

    let randao_verification = get_randao_verification(&query, randao_reveal.is_infinity())?;
    let block_response_type = chain
        .produce_block_with_verification(
            randao_reveal,
            slot,
            query.graffiti.map(Into::into),
            randao_verification,
            None,
            BlockProductionVersion::BlindedV2,
        )
        .await
        .map_err(|e| AxumError::ServerError(format!("Block production error: {:?}", e)))?;

    build_response_v2(chain, block_response_type, endpoint_version, accept_header)
}

pub async fn produce_block_v2<T: BeaconChainTypes>(
    State(chain): State<Arc<BeaconChain<T>>>,
    Query(query): Query<api_types::ValidatorBlocksQuery>,
    Path(slot): Path<Slot>,
    endpoint_version: EndpointVersion,
    accept_header: Option<api_types::Accept>,
) -> Result<impl IntoResponse, AxumError> {
    let randao_reveal = query.randao_reveal.decompress().map_err(|e| {
        AxumError::BadRequest(format!(
            "randao reveal is not a valid BLS signature: {:?}",
            e
        ))
    })?;

    let randao_verification = get_randao_verification(&query, randao_reveal.is_infinity())?;

    let block_response_type = chain
        .produce_block_with_verification(
            randao_reveal,
            slot,
            query.graffiti.map(Into::into),
            randao_verification,
            None,
            BlockProductionVersion::FullV2,
        )
        .await
        .map_err(|e| AxumError::BlockProductionError(format!("Block production error: {:?}", e)))?;

    build_response_v2(chain, block_response_type, endpoint_version, accept_header)
}

pub fn build_response_v2<T: BeaconChainTypes>(
    chain: Arc<BeaconChain<T>>,
    block_response: BeaconBlockResponseWrapper<T::EthSpec>,
    endpoint_version: EndpointVersion,
    accept_header: Option<api_types::Accept>,
) -> Result<Response<Body>, AxumError> {
    let fork_name = block_response
        .fork_name(&chain.spec)
        .map_err(|e| AxumError::ServerError(format!("Inconsistent fork: {:?}", e)))?;

    let block_contents = build_block_contents::build_block_contents(fork_name, block_response)?;

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .unwrap();

    response = add_consensus_version_header(response, fork_name)?;

    match accept_header {
        Some(api_types::Accept::Ssz) => {
            response = add_ssz_content_type_header(response);
            *response.body_mut() = Body::from(block_contents.as_ssz_bytes());
            Ok(response)
        }
        _ => {
            response.headers_mut().insert(CONTENT_TYPE, "application/json".parse().unwrap());
            let json_response = fork_versioned_response(endpoint_version, fork_name, block_contents)?;
            *response.body_mut() = Body::from(serde_json::to_vec(&json_response)?);
            Ok(response)
        }
    }
}