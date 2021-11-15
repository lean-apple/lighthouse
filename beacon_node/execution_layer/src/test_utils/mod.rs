//! Provides a mock execution engine HTTP JSON-RPC API for use in testing.

use crate::engine_api::http::JSONRPC_VERSION;
use crate::engine_api::ExecutePayloadResponse;
use bytes::Bytes;
use environment::null_logger;
use handle_rpc::handle_rpc;
use parking_lot::{Mutex, RwLock, RwLockWriteGuard};
use serde::{Deserialize, Serialize};
use serde_json::json;
use slog::{info, Logger};
use std::future::Future;
use std::marker::PhantomData;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::{runtime, sync::oneshot};
use types::{EthSpec, Hash256, Uint256};
use warp::Filter;

pub use execution_block_generator::{generate_pow_block, ExecutionBlockGenerator};
pub use mock_execution_layer::{ExecutionLayerRuntime, MockExecutionLayer};

pub const DEFAULT_TERMINAL_DIFFICULTY: u64 = 6400;
pub const DEFAULT_TERMINAL_BLOCK: u64 = 64;

mod execution_block_generator;
mod handle_rpc;
mod mock_execution_layer;

pub struct MockServer<T: EthSpec> {
    _shutdown_tx: oneshot::Sender<()>,
    listen_socket_addr: SocketAddr,
    last_echo_request: Arc<RwLock<Option<Bytes>>>,
    pub ctx: Arc<Context<T>>,
}

impl<T: EthSpec> MockServer<T> {
    pub fn unit_testing() -> Self {
        Self::new(
            &runtime::Handle::current(),
            DEFAULT_TERMINAL_DIFFICULTY.into(),
            DEFAULT_TERMINAL_BLOCK,
            Hash256::zero(),
        )
    }

    pub fn new(
        handle: &runtime::Handle,
        terminal_difficulty: Uint256,
        terminal_block: u64,
        terminal_block_hash: Hash256,
    ) -> Self {
        let last_echo_request = Arc::new(RwLock::new(None));
        let preloaded_responses = Arc::new(Mutex::new(vec![]));
        let execution_block_generator =
            ExecutionBlockGenerator::new(terminal_difficulty, terminal_block, terminal_block_hash);

        let ctx: Arc<Context<T>> = Arc::new(Context {
            config: <_>::default(),
            log: null_logger().unwrap(),
            last_echo_request: last_echo_request.clone(),
            execution_block_generator: RwLock::new(execution_block_generator),
            preloaded_responses,
            static_execute_payload_response: <_>::default(),
            _phantom: PhantomData,
        });

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let shutdown_future = async {
            // Ignore the result from the channel, shut down regardless.
            let _ = shutdown_rx.await;
        };

        // The `serve` function will panic unless it's run inside a tokio runtime, so use `block_on`
        // if we're not in a runtime. However, we can't *always* use `block_on` since tokio will
        // panic if we try to block inside an async context.
        let serve = || serve(ctx.clone(), shutdown_future).unwrap();
        let (listen_socket_addr, server_future) = if runtime::Handle::try_current().is_err() {
            handle.block_on(async { serve() })
        } else {
            serve()
        };

        handle.spawn(server_future);

        Self {
            _shutdown_tx: shutdown_tx,
            listen_socket_addr,
            last_echo_request,
            ctx,
        }
    }

    pub fn execution_block_generator(&self) -> RwLockWriteGuard<'_, ExecutionBlockGenerator<T>> {
        self.ctx.execution_block_generator.write()
    }

    pub fn url(&self) -> String {
        format!(
            "http://{}:{}",
            self.listen_socket_addr.ip(),
            self.listen_socket_addr.port()
        )
    }

    pub fn last_echo_request(&self) -> Bytes {
        self.last_echo_request
            .write()
            .take()
            .expect("last echo request is none")
    }

    pub fn push_preloaded_response(&self, response: serde_json::Value) {
        self.ctx.preloaded_responses.lock().push(response)
    }

    pub fn all_payloads_valid(&self) {
        *self.ctx.static_execute_payload_response.lock() = Some(ExecutePayloadResponse::Valid)
    }
}

#[derive(Debug)]
pub enum Error {
    Warp(warp::Error),
    Other(String),
}

impl From<warp::Error> for Error {
    fn from(e: warp::Error) -> Self {
        Error::Warp(e)
    }
}

impl From<String> for Error {
    fn from(e: String) -> Self {
        Error::Other(e)
    }
}

#[derive(Debug)]
struct MissingIdField;

impl warp::reject::Reject for MissingIdField {}

/// A wrapper around all the items required to spawn the HTTP server.
///
/// The server will gracefully handle the case where any fields are `None`.
pub struct Context<T: EthSpec> {
    pub config: Config,
    pub log: Logger,
    pub last_echo_request: Arc<RwLock<Option<Bytes>>>,
    pub execution_block_generator: RwLock<ExecutionBlockGenerator<T>>,
    pub preloaded_responses: Arc<Mutex<Vec<serde_json::Value>>>,
    pub static_execute_payload_response: Arc<Mutex<Option<ExecutePayloadResponse>>>,
    pub _phantom: PhantomData<T>,
}

/// Configuration for the HTTP server.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub listen_addr: Ipv4Addr,
    pub listen_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_addr: Ipv4Addr::new(127, 0, 0, 1),
            listen_port: 0,
        }
    }
}

/// Creates a server that will serve requests using information from `ctx`.
///
/// The server will shut down gracefully when the `shutdown` future resolves.
///
/// ## Returns
///
/// This function will bind the server to the provided address and then return a tuple of:
///
/// - `SocketAddr`: the address that the HTTP server will listen on.
/// - `Future`: the actual server future that will need to be awaited.
///
/// ## Errors
///
/// Returns an error if the server is unable to bind or there is another error during
/// configuration.
pub fn serve<T: EthSpec>(
    ctx: Arc<Context<T>>,
    shutdown: impl Future<Output = ()> + Send + Sync + 'static,
) -> Result<(SocketAddr, impl Future<Output = ()>), Error> {
    let config = &ctx.config;
    let log = ctx.log.clone();

    let inner_ctx = ctx.clone();
    let ctx_filter = warp::any().map(move || inner_ctx.clone());

    // `/`
    //
    // Handles actual JSON-RPC requests.
    let root = warp::path::end()
        .and(warp::body::json())
        .and(ctx_filter.clone())
        .and_then(|body: serde_json::Value, ctx: Arc<Context<T>>| async move {
            let id = body
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| warp::reject::custom(MissingIdField))?;

            let preloaded_response = {
                let mut preloaded_responses = ctx.preloaded_responses.lock();
                if !preloaded_responses.is_empty() {
                    Some(preloaded_responses.remove(0))
                } else {
                    None
                }
            };

            let response = if let Some(preloaded_response) = preloaded_response {
                preloaded_response
            } else {
                match handle_rpc(body, ctx).await {
                    Ok(result) => json!({
                        "id": id,
                        "jsonrpc": JSONRPC_VERSION,
                        "result": result
                    }),
                    Err(message) => json!({
                        "id": id,
                        "jsonrpc": JSONRPC_VERSION,
                        "error": {
                            "code": -1234,   // Junk error code.
                            "message": message
                        }
                    }),
                }
            };

            Ok::<_, warp::reject::Rejection>(
                warp::http::Response::builder()
                    .status(200)
                    .body(serde_json::to_string(&response).expect("response must be valid JSON")),
            )
        });

    // `/echo`
    //
    // Sends the body of the request to `ctx.last_echo_request` so we can inspect requests.
    let echo = warp::path("echo")
        .and(warp::body::bytes())
        .and(ctx_filter)
        .and_then(|bytes: Bytes, ctx: Arc<Context<T>>| async move {
            *ctx.last_echo_request.write() = Some(bytes.clone());
            Ok::<_, warp::reject::Rejection>(
                warp::http::Response::builder().status(200).body(bytes),
            )
        });

    let routes = warp::post()
        .and(root.or(echo))
        // Add a `Server` header.
        .map(|reply| warp::reply::with_header(reply, "Server", "lighthouse-mock-execution-client"));

    let (listening_socket, server) = warp::serve(routes).try_bind_with_graceful_shutdown(
        SocketAddrV4::new(config.listen_addr, config.listen_port),
        async {
            shutdown.await;
        },
    )?;

    info!(
        log,
        "Metrics HTTP server started";
        "listen_address" => listening_socket.to_string(),
    );

    Ok((listening_socket, server))
}
