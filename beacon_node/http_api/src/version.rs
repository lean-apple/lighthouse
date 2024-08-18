use crate::api_types::EndpointVersion;
use eth2::{
    CONSENSUS_BLOCK_VALUE_HEADER, CONSENSUS_VERSION_HEADER, CONTENT_TYPE_HEADER,
    EXECUTION_PAYLOAD_BLINDED_HEADER, EXECUTION_PAYLOAD_VALUE_HEADER, SSZ_CONTENT_TYPE_HEADER,
};
use serde::Serialize;
use types::{
    fork_versioned_response::{
        ExecutionOptimisticFinalizedForkVersionedResponse, ExecutionOptimisticFinalizedMetadata,
    },
    ForkName, ForkVersionedResponse, InconsistentFork, Uint256,
};
use axum::{http::HeaderValue, response::Response};

use crate::axum_server::error::Error as AxumError;

pub const V1: EndpointVersion = EndpointVersion(1);
pub const V2: EndpointVersion = EndpointVersion(2);
pub const V3: EndpointVersion = EndpointVersion(3);

pub fn fork_versioned_response<T: Serialize>(
    endpoint_version: EndpointVersion,
    fork_name: ForkName,
    data: T,
) -> Result<ForkVersionedResponse<T>, AxumError> {
    let fork_name = if endpoint_version == V1 {
        None
    } else if endpoint_version == V2 || endpoint_version == V3 {
        Some(fork_name)
    } else {
        return Err(AxumError::UnsupportedVersion(endpoint_version));
    };
    Ok(ForkVersionedResponse {
        version: fork_name,
        metadata: Default::default(),
        data,
    })
}

pub fn execution_optimistic_finalized_fork_versioned_response<T: Serialize>(
    endpoint_version: EndpointVersion,
    fork_name: ForkName,
    execution_optimistic: bool,
    finalized: bool,
    data: T,
) -> Result<ExecutionOptimisticFinalizedForkVersionedResponse<T>, AxumError> {
    let fork_name = if endpoint_version == V1 {
        None
    } else if endpoint_version == V2 {
        Some(fork_name)
    } else {
        return Err(AxumError::UnsupportedVersion(endpoint_version));
    };
    Ok(ExecutionOptimisticFinalizedForkVersionedResponse {
        version: fork_name,
        metadata: ExecutionOptimisticFinalizedMetadata {
            execution_optimistic: Some(execution_optimistic),
            finalized: Some(finalized),
        },
        data,
    })
}

/// Add the 'Content-Type application/octet-stream` header to a response.
pub fn add_ssz_content_type_header(mut response: Response) -> Response {
    response.headers_mut().insert(
        CONTENT_TYPE_HEADER,
        HeaderValue::from_static(SSZ_CONTENT_TYPE_HEADER),
    );
    response
}
/// Add the `Eth-Consensus-Version` header to a response.
pub fn add_consensus_version_header(
    mut response: Response,
    fork_name: ForkName,
) -> Result<Response, AxumError> {
    response.headers_mut().insert(
        CONSENSUS_VERSION_HEADER,
        HeaderValue::from_str(&fork_name.to_string())
            .map_err(|e| AxumError::BadRequest(format!("Invalid fork name: {}", e)))?,
    );
    Ok(response)
}

/// Add the `Eth-Execution-Payload-Blinded` header to a response.
pub fn add_execution_payload_blinded_header(
    mut response: Response,
    execution_payload_blinded: bool,
) -> Result<Response, AxumError> {
    response.headers_mut().insert(
        EXECUTION_PAYLOAD_BLINDED_HEADER,
        HeaderValue::from_str(&execution_payload_blinded.to_string()).map_err(|e| {
            AxumError::BadRequest(format!("Invalid execution payload blinded value: {}", e))
        })?,
    );
    Ok(response)
}

/// Add the `Eth-Execution-Payload-Value` header to a response.
pub fn add_execution_payload_value_header(
    mut response: Response,
    execution_payload_value: Uint256,
) -> Result<Response, AxumError> {
    response.headers_mut().insert(
        EXECUTION_PAYLOAD_VALUE_HEADER,
        HeaderValue::from_str(&execution_payload_value.to_string()).map_err(|e| {
            AxumError::BadRequest(format!("Invalid execution payload value: {}", e))
        })?,
    );
    Ok(response)
}

/// Add the `Eth-Consensus-Block-Value` header to a response.
pub fn add_consensus_block_value_header(
    mut response: Response,
    consensus_payload_value: Uint256,
) -> Result<Response, AxumError> {
    response.headers_mut().insert(
        CONSENSUS_BLOCK_VALUE_HEADER,
        HeaderValue::from_str(&consensus_payload_value.to_string()).map_err(|e| {
            AxumError::BadRequest(format!("Invalid consensus payload value: {}", e))
        })?,
    );
    Ok(response)
}

pub fn inconsistent_fork_rejection(error: InconsistentFork) -> AxumError {
    AxumError::InconsistentFork(error)
}

pub fn unsupported_version_rejection(version: EndpointVersion) -> AxumError {
    AxumError::UnsupportedVersion(version)
}
