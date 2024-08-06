use axum::{
    response::{IntoResponse, Response},
    Json,
    http::StatusCode,
    Error as AxumError,
    extract::rejection::{
        ExtensionRejection, 
        FormRejection,
        JsonRejection,
        PathRejection,
        QueryRejection,
    },
};
use serde_json::json;
use std::io::Error as IoError;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    BadRequest(String),
    ServerError(String),
    Other(String),
    Axum(AxumError),
    ExtensionError(ExtensionRejection),
    FormError(FormRejection),
    IoError(IoError),
    JsonError(JsonRejection),
    QueryError(QueryRejection),
    PathError(PathRejection),
    BeaconChainError(String),
    InvalidRandaoReveal(String),
    BlockProductionError(String),
    InconsistentFork(String),
    NotFound(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(msg) => write!(f, "Bad Request: {}", msg),
            Self::ServerError(msg) => write!(f, "Custom Server Error: {}", msg),
            Self::Other(msg) => write!(f, "Other Error: {}", msg),
            Self::Axum(e) => write!(f, "Axum Error: {:?}", e),
            Self::ExtensionError(e) => write!(f, "Extension Error: {:?}", e),
            Self::FormError(e) => write!(f, "Form Error: {:?}", e),
            Self::IoError(e) => write!(f, "IO Error: {}", e),
            Self::JsonError(e) => write!(f, "JSON Error: {:?}", e),
            Self::QueryError(e) => write!(f, "Query Error: {:?}", e),
            Self::PathError(e) => write!(f, "Path Error: {:?}", e),
            Self::BeaconChainError(msg) => write!(f, "Beacon Chain Error: {}", msg),
            Self::InvalidRandaoReveal(msg) => write!(f, "Invalid RANDAO Reveal: {}", msg),
            Self::BlockProductionError(msg) => write!(f, "Block Production Error: {}", msg),
            Self::InconsistentFork(msg) => write!(f, "Inconsistent Fork: {}", msg),
            Self::NotFound(msg) => write!(f, "Not Found: {}", msg),
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Self::ServerError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Self::Axum(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Axum internal error".to_string()),
            Self::QueryError(_) => (StatusCode::BAD_REQUEST, "Invalid query parameters".to_string()),
            Self::PathError(_) => (StatusCode::BAD_REQUEST, "Invalid path parameters".to_string()),
            Self::JsonError(_) => (StatusCode::BAD_REQUEST, "Error in JSON payload".to_string()),
            Self::FormError(_) => (StatusCode::BAD_REQUEST, "Error in form data".to_string()),
            Self::ExtensionError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Server misconfiguration".to_string()),
            Self::BeaconChainError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Beacon chain error: {}", msg)),
            Self::InvalidRandaoReveal(msg) => (StatusCode::BAD_REQUEST, format!("Invalid RANDAO reveal: {}", msg)),
            Self::BlockProductionError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Block production error: {}", msg)),
            Self::InconsistentFork(msg) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Inconsistent fork: {}", msg)),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error".to_string()),
        };
        (status, Json(json!({ "error": error_message }))).into_response()
    }
}