use beacon_chain::store::metadata::CURRENT_SCHEMA_VERSION;
use beacon_chain::{BeaconChain, BeaconChainTypes};
use eth2::lighthouse::DatabaseInfo;
use std::sync::Arc;
use axum::Json;
use crate::axum_server::error::Error as AxumError;

pub async fn info<T: BeaconChainTypes>(
    chain: Arc<BeaconChain<T>>,
) -> Result<Json<DatabaseInfo>, AxumError> {
    let store = &chain.store;
    let split = store.get_split_info();
    let config = store.get_config().clone();
    let anchor = store.get_anchor_info();
    let blob_info = store.get_blob_info();

    Ok(Json(DatabaseInfo {
        schema_version: CURRENT_SCHEMA_VERSION.as_u64(),
        config,
        split,
        anchor,
        blob_info,
    }))
}