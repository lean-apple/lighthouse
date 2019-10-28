extern crate slog;

mod config;

pub mod builder;
pub mod error;

use beacon_chain::BeaconChain;
use exit_future::Signal;
use network::Service as NetworkService;
use std::net::SocketAddr;
use std::sync::Arc;

pub use beacon_chain::{builder::BeaconChainStartMethod, BeaconChainTypes, Eth1ChainBackend};
pub use builder::ClientBuilder;
pub use config::{ClientGenesis, Config as ClientConfig};
pub use eth2_config::Eth2Config;

/// The core "beacon node" client.
///
/// Holds references to running services, cleanly shutting them down when dropped.
pub struct Client<T: BeaconChainTypes> {
    beacon_chain: Option<Arc<BeaconChain<T>>>,
    libp2p_network: Option<Arc<NetworkService<T>>>,
    http_listen_addr: Option<SocketAddr>,
    websocket_listen_addr: Option<SocketAddr>,
    /// Exit signals will "fire" when dropped, causing each service to exit gracefully.
    _exit_signals: Vec<Signal>,
}

impl<T: BeaconChainTypes> Client<T> {
    /// Returns an `Arc` reference to the client's `BeaconChain`, if it was started.
    pub fn beacon_chain(&self) -> Option<Arc<BeaconChain<T>>> {
        self.beacon_chain.clone()
    }

    /// Returns the address of the client's HTTP API server, if it was started.
    pub fn http_listen_addr(&self) -> Option<SocketAddr> {
        self.http_listen_addr
    }

    /// Returns the address of the client's WebSocket API server, if it was started.
    pub fn websocket_listen_addr(&self) -> Option<SocketAddr> {
        self.websocket_listen_addr
    }

    /// Returns the port of the client's libp2p stack, if it was started.
    pub fn libp2p_listen_port(&self) -> Option<u16> {
        self.libp2p_network.as_ref().map(|n| n.listen_port())
    }
}

impl<T: BeaconChainTypes> Drop for Client<T> {
    fn drop(&mut self) {
        if let Some(beacon_chain) = &self.beacon_chain {
            let _result = beacon_chain.persist();
        }
    }
}
