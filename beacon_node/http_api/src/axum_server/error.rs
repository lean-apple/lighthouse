use crate::EndpointVersion;
use axum::{
    extract::rejection::{
        ExtensionRejection, FormRejection, JsonRejection, PathRejection, QueryRejection,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    Error as AxumError, Json,
};
use serde_json::json;
use std::fmt;
use std::io::Error as IoError;
use types::fork_name::InconsistentFork;

#[derive(Debug)]
pub enum Error {
    BadRequest(String),
    ServerError(String),
    NotFound(String),
    Other(String),
    Axum(AxumError),
    ExtensionError(ExtensionRejection),
    FormError(FormRejection),
    IoError(IoError),
    JsonError(JsonRejection),
    QueryError(QueryRejection),
    PathError(PathRejection),
    BeaconChainError(String),
    BeaconStateError(String),
    InvalidRandaoReveal(String),
    SlotProcessingError(String),
    BlockProductionError(String),
    InconsistentFork(InconsistentFork),
    ArithError(String),
    DeserializeError(String),
    BroadcastWithoutImport(String),
    ObjectInvalid(String),
    NotSynced(String),
    InvalidAuthorization(String),
    IndexedBadRequestErrors {
        message: String,
        failures: Vec<String>,
    },
    UnsupportedVersion(EndpointVersion),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(msg) => write!(f, "Bad Request: {}", msg),
            Self::ServerError(msg) => write!(f, "Server Error: {}", msg),
            Self::NotFound(msg) => write!(f, "Not Found: {}", msg),
            Self::Other(msg) => write!(f, "Other Error: {}", msg),
            Self::Axum(e) => write!(f, "Axum Error: {:?}", e),
            Self::ExtensionError(e) => write!(f, "Extension Error: {:?}", e),
            Self::FormError(e) => write!(f, "Form Error: {:?}", e),
            Self::IoError(e) => write!(f, "IO Error: {}", e),
            Self::JsonError(e) => write!(f, "JSON Error: {:?}", e),
            Self::QueryError(e) => write!(f, "Query Error: {:?}", e),
            Self::PathError(e) => write!(f, "Path Error: {:?}", e),
            Self::BeaconChainError(msg) => write!(f, "Beacon Chain Error: {}", msg),
            Self::BeaconStateError(msg) => write!(f, "Beacon State Error: {}", msg),
            Self::InvalidRandaoReveal(msg) => write!(f, "Invalid RANDAO Reveal: {}", msg),
            Self::InconsistentFork(msg) => write!(f, "Inconsistent Fork: {}", msg),
            Self::SlotProcessingError(msg) => write!(f, "Slot Processing Error: {}", msg),
            Self::BlockProductionError(msg) => write!(f, "Block Production Error: {}", msg),
            Self::ArithError(msg) => write!(f, "Arithmetic Error: {}", msg),
            Self::DeserializeError(msg) => write!(f, "Deserialize Error: {}", msg),
            Self::BroadcastWithoutImport(msg) => write!(f, "Broadcast Without Import: {}", msg),
            Self::ObjectInvalid(msg) => write!(f, "Object Invalid: {}", msg),
            Self::NotSynced(msg) => write!(f, "Not Synced: {}", msg),
            Self::InvalidAuthorization(msg) => write!(f, "Invalid Authorization: {}", msg),
            Self::IndexedBadRequestErrors { message, failures } => write!(
                f,
                "Indexed Bad Request Errors: {}, Failures: {:?}",
                message, failures
            ),
            Self::InconsistentFork(error) => write!(f, "Inconsistent Fork: {:?}", error),
            Self::UnsupportedVersion(version) => write!(f, "Unsupported Version: {}", version),
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::ServerError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::Other(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Self::Axum(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Axum internal error".to_string(),
            ),
            Self::QueryError(_) => (
                StatusCode::BAD_REQUEST,
                "Invalid query parameters".to_string(),
            ),
            Self::PathError(_) => (
                StatusCode::BAD_REQUEST,
                "Invalid path parameters".to_string(),
            ),
            Self::JsonError(_) => (StatusCode::BAD_REQUEST, "Error in JSON payload".to_string()),
            Self::FormError(_) => (StatusCode::BAD_REQUEST, "Error in form data".to_string()),
            Self::ExtensionError(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Server misconfiguration".to_string(),
            ),
            Self::IoError(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("IO Error: {}", e),
            ),
            Self::BeaconChainError(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Beacon chain error: {}", msg),
            ),
            Self::BeaconStateError(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Beacon state error: {}", msg),
            ),
            Self::InvalidRandaoReveal(msg) => (
                StatusCode::BAD_REQUEST,
                format!("Invalid RANDAO reveal: {}", msg),
            ),
            Self::InconsistentFork(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Inconsistent fork: {}", msg),
            ),
            Self::SlotProcessingError(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Slot processing error: {}", msg),
            ),
            Self::BlockProductionError(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Block production error: {}", msg),
            ),
            Self::ArithError(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Arithmetic error: {}", msg),
            ),
            Self::DeserializeError(msg) => (
                StatusCode::BAD_REQUEST,
                format!("Deserialize error: {}", msg),
            ),
            Self::BroadcastWithoutImport(msg) => (
                StatusCode::ACCEPTED,
                format!("Broadcast without import: {}", msg),
            ),
            Self::ObjectInvalid(msg) => {
                (StatusCode::BAD_REQUEST, format!("Invalid object: {}", msg))
            }
            Self::NotSynced(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Not synced: {}", msg),
            ),
            Self::InconsistentFork(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Inconsistent fork: {:?}", error),
            ),
            Self::UnsupportedVersion(version) => (
                StatusCode::BAD_REQUEST,
                format!("Unsupported endpoint version: {}", version),
            ),
            Self::InvalidAuthorization(msg) => (
                StatusCode::FORBIDDEN,
                format!("Invalid authorization: {}", msg),
            ),
            Self::IndexedBadRequestErrors { message, failures } => (
                StatusCode::BAD_REQUEST,
                format!(
                    "Indexed bad request errors: {}, Failures: {:?}",
                    message, failures
                ),
            ),
        };

        let body = Json(json!({
            "error": {
                "message": error_message,
                "code": status.as_u16()
            }
        }));

        (status, body).into_response()
    }
}