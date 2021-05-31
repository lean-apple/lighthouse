//! This service keeps track of which shard subnet the beacon node should be subscribed to at any
//! given time. It schedules subscriptions to shard subnets, requests peer discoveries and
//! determines whether attestations should be aggregated and/or passed to the beacon node.

pub mod attestation_subnets;
pub mod sync_subnets;

use eth2_libp2p::SubnetDiscovery;
use types::SubnetId;

pub use attestation_subnets::AttestationService;
pub use sync_subnets::SyncCommitteeService;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum SubnetServiceMessage {
    /// Subscribe to the specified subnet id.
    Subscribe(SubnetId),
    /// Unsubscribe to the specified subnet id.
    Unsubscribe(SubnetId),
    /// Add the `SubnetId` to the ENR bitfield.
    EnrAdd(SubnetId),
    /// Remove the `SubnetId` from the ENR bitfield.
    EnrRemove(SubnetId),
    /// Discover peers for a list of `SubnetDiscovery`.
    DiscoverPeers(Vec<SubnetDiscovery>),
}

/// Note: This `PartialEq` impl is for use only in tests.
/// The `DiscoverPeers` comparison is good enough for testing only.
#[cfg(test)]
impl PartialEq for SubnetServiceMessage {
    fn eq(&self, other: &SubnetServiceMessage) -> bool {
        match (self, other) {
            (SubnetServiceMessage::Subscribe(a), SubnetServiceMessage::Subscribe(b)) => a == b,
            (SubnetServiceMessage::Unsubscribe(a), SubnetServiceMessage::Unsubscribe(b)) => a == b,
            (SubnetServiceMessage::EnrAdd(a), SubnetServiceMessage::EnrAdd(b)) => a == b,
            (SubnetServiceMessage::EnrRemove(a), SubnetServiceMessage::EnrRemove(b)) => a == b,
            (SubnetServiceMessage::DiscoverPeers(a), SubnetServiceMessage::DiscoverPeers(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                for i in 0..a.len() {
                    if a[i].subnet != b[i].subnet || a[i].min_ttl != b[i].min_ttl {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }
}
