use derivative::Derivative;
use slot_clock::SlotClock;
use std::sync::Arc;

use crate::beacon_chain::{
    BeaconChain, BeaconChainTypes, MAXIMUM_GOSSIP_CLOCK_DISPARITY,
    VALIDATOR_PUBKEY_CACHE_LOCK_TIMEOUT,
};
use crate::gossip_blob_cache::BlobCacheError;
use crate::BeaconChainError;
use state_processing::per_block_processing::eip4844::eip4844::verify_kzg_commitments_against_transactions;
use types::{
    BeaconBlockRef, BeaconStateError, BlobSidecar, BlobSidecarList, Epoch, EthSpec, Hash256,
    KzgCommitment, SignedBeaconBlock, SignedBeaconBlockHeader, SignedBlobSidecar, Slot,
    Transactions,
};

#[derive(Debug)]
pub enum BlobError {
    /// The blob sidecar is from a slot that is later than the current slot (with respect to the
    /// gossip clock disparity).
    ///
    /// ## Peer scoring
    ///
    /// Assuming the local clock is correct, the peer has sent an invalid message.
    FutureSlot {
        message_slot: Slot,
        latest_permissible_slot: Slot,
    },

    /// The blob sidecar has a different slot than the block.
    ///
    /// ## Peer scoring
    ///
    /// Assuming the local clock is correct, the peer has sent an invalid message.
    SlotMismatch {
        blob_slot: Slot,
        block_slot: Slot,
    },

    /// No kzg ccommitment associated with blob sidecar.
    KzgCommitmentMissing,

    /// No transactions in block
    TransactionsMissing,

    /// Blob transactions in the block do not correspond to the kzg commitments.
    TransactionCommitmentMismatch,

    TrustedSetupNotInitialized,

    InvalidKzgProof,

    KzgError(kzg::Error),

    /// There was an error whilst processing the sync contribution. It is not known if it is valid or invalid.
    ///
    /// ## Peer scoring
    ///
    /// We were unable to process this sync committee message due to an internal error. It's unclear if the
    /// sync committee message is valid.
    BeaconChainError(BeaconChainError),
    /// No blobs for the specified block where we would expect blobs.
    UnavailableBlobs,
    /// Blobs provided for a pre-Eip4844 fork.
    InconsistentFork,

    /// The `blobs_sidecar.message.beacon_block_root` block is unknown.
    ///
    /// ## Peer scoring
    ///
    /// The blob points to a block we have not yet imported. The blob cannot be imported
    /// into fork choice yet
    UnknownHeadBlock {
        beacon_block_root: Hash256,
    },

    /// The `BlobSidecar` was gossiped over an incorrect subnet.
    InvalidSubnet {
        expected: u64,
        received: u64,
    },

    /// The sidecar corresponds to a slot older than the finalized head slot.
    PastFinalizedSlot {
        blob_slot: Slot,
        finalized_slot: Slot,
    },

    /// The proposer index specified in the sidecar does not match the locally computed
    /// proposer index.
    ProposerIndexMismatch {
        sidecar: usize,
        local: usize,
    },

    ProposerSignatureInvalid,

    /// A sidecar with same slot, beacon_block_root and proposer_index but different blob is received for
    /// the same blob index.
    RepeatSidecar {
        proposer: usize,
        slot: Slot,
        blob_index: usize,
    },

    /// The proposal_index corresponding to blob.beacon_block_root is not known.
    ///
    /// ## Peer scoring
    ///
    /// The block is invalid and the peer is faulty.
    UnknownValidator(u64),

    BlobCacheError(BlobCacheError),
}

impl From<BeaconChainError> for BlobError {
    fn from(e: BeaconChainError) -> Self {
        BlobError::BeaconChainError(e)
    }
}

impl From<BeaconStateError> for BlobError {
    fn from(e: BeaconStateError) -> Self {
        BlobError::BeaconChainError(BeaconChainError::BeaconStateError(e))
    }
}

pub struct GossipVerifiedBlobSidecar {
    /// Indicates if all blobs for a given block_root are available
    /// in the blob cache.
    pub all_blobs_available: bool,
    pub block_root: Hash256,
}

pub fn validate_blob_sidecar_for_gossip<T: BeaconChainTypes>(
    signed_blob_sidecar: Arc<SignedBlobSidecar<T::EthSpec>>,
    subnet: u64,
    chain: &BeaconChain<T>,
) -> Result<GossipVerifiedBlobSidecar, BlobError> {
    let blob_slot = signed_blob_sidecar.message.slot;
    let blob_index = signed_blob_sidecar.message.index;
    let block_root = signed_blob_sidecar.message.block_root;

    // Verify that the blob_sidecar was received on the correct subnet.
    if blob_index != subnet {
        return Err(BlobError::InvalidSubnet {
            expected: blob_index,
            received: subnet,
        });
    }

    // Verify that the sidecar is not from a future slot.
    let latest_permissible_slot = chain
        .slot_clock
        .now_with_future_tolerance(MAXIMUM_GOSSIP_CLOCK_DISPARITY)
        .ok_or(BeaconChainError::UnableToReadSlot)?;
    if blob_slot > latest_permissible_slot {
        return Err(BlobError::FutureSlot {
            message_slot: blob_slot,
            latest_permissible_slot,
        });
    }

    // TODO(pawan): Verify not from a past slot?

    // Verify that the sidecar slot is greater than the latest finalized slot
    let latest_finalized_slot = chain
        .head()
        .finalized_checkpoint()
        .epoch
        .start_slot(T::EthSpec::slots_per_epoch());
    if blob_slot <= latest_finalized_slot {
        return Err(BlobError::PastFinalizedSlot {
            blob_slot,
            finalized_slot: latest_finalized_slot,
        });
    }

    // TODO(pawan): should we verify locally that the parent root is correct
    // or just use whatever the proposer gives us?
    let proposer_shuffling_root = signed_blob_sidecar.message.block_parent_root;

    let (proposer_index, fork) = match chain
        .beacon_proposer_cache
        .lock()
        .get_slot::<T::EthSpec>(proposer_shuffling_root, blob_slot)
    {
        Some(proposer) => (proposer.index, proposer.fork),
        None => {
            let state = &chain.canonical_head.cached_head().snapshot.beacon_state;
            (
                state.get_beacon_proposer_index(blob_slot, &chain.spec)?,
                state.fork(),
            )
        }
    };

    let blob_proposer_index = signed_blob_sidecar.message.proposer_index;
    if proposer_index != blob_proposer_index as usize {
        return Err(BlobError::ProposerIndexMismatch {
            sidecar: blob_proposer_index as usize,
            local: proposer_index,
        });
    }

    let signature_is_valid = {
        let pubkey_cache = chain
            .validator_pubkey_cache
            .try_read_for(VALIDATOR_PUBKEY_CACHE_LOCK_TIMEOUT)
            .ok_or(BeaconChainError::ValidatorPubkeyCacheLockTimeout)
            .map_err(BlobError::BeaconChainError)?;

        let pubkey = pubkey_cache
            .get(proposer_index as usize)
            .ok_or_else(|| BlobError::UnknownValidator(proposer_index as u64))?;

        signed_blob_sidecar.verify_signature(
            None,
            pubkey,
            &fork,
            chain.genesis_validators_root,
            &chain.spec,
        )
    };

    if !signature_is_valid {
        return Err(BlobError::ProposerSignatureInvalid);
    }

    // TODO(pawan): kzg validations.

    // TODO(pawan): Check if other blobs for the same proposer index and blob index have been
    // received and drop if required.

    let da_checker = chain.data_availability_checker.as_ref().unwrap();
    let all_blobs_available = da_checker
        .put_blob_temp(signed_blob_sidecar)
        .map_err(BlobError::BlobCacheError)?;

    // Verify if the corresponding block for this blob has been received.
    // Note: this should be the last gossip check so that we can forward the blob
    // over the gossip network even if we haven't received the corresponding block yet
    // as all other validations have passed.
    let block_opt = chain
        .canonical_head
        .fork_choice_read_lock()
        .get_block(&block_root)
        .or_else(|| chain.early_attester_cache.get_proto_block(block_root)); // TODO(pawan): should we be checking this cache?

    if block_opt.is_none() {
        return Err(BlobError::UnknownHeadBlock {
            beacon_block_root: block_root,
        });
    }

    Ok(GossipVerifiedBlobSidecar {
        all_blobs_available,
        block_root,
    })
}

pub fn verify_data_availability<T: BeaconChainTypes>(
    blob_sidecar: &BlobSidecarList<T::EthSpec>,
    kzg_commitments: &[KzgCommitment],
    transactions: &Transactions<T::EthSpec>,
    _block_slot: Slot,
    _block_root: Hash256,
    chain: &BeaconChain<T>,
) -> Result<(), BlobError> {
    if verify_kzg_commitments_against_transactions::<T::EthSpec>(transactions, kzg_commitments)
        .is_err()
    {
        return Err(BlobError::TransactionCommitmentMismatch);
    }

    // Validatate that the kzg proof is valid against the commitments and blobs
    let _kzg = chain
        .kzg
        .as_ref()
        .ok_or(BlobError::TrustedSetupNotInitialized)?;

    todo!("use `kzg_utils::validate_blobs` once the function is updated")
    // if !kzg_utils::validate_blobs_sidecar(
    //     kzg,
    //     block_slot,
    //     block_root,
    //     kzg_commitments,
    //     blob_sidecar,
    // )
    // .map_err(BlobError::KzgError)?
    // {
    //     return Err(BlobError::InvalidKzgProof);
    // }
    // Ok(())
}

#[derive(Copy, Clone)]
pub enum DataAvailabilityCheckRequired {
    Yes,
    No,
}

pub trait IntoAvailableBlock<T: BeaconChainTypes> {
    fn into_available_block(
        self,
        block_root: Hash256,
        chain: &BeaconChain<T>,
    ) -> Result<AvailableBlock<T::EthSpec>, BlobError>;
}

impl<T: BeaconChainTypes> IntoAvailableBlock<T> for BlockWrapper<T::EthSpec> {
    fn into_available_block(
        self,
        block_root: Hash256,
        chain: &BeaconChain<T>,
    ) -> Result<AvailableBlock<T::EthSpec>, BlobError> {
        todo!()
    }
}

#[derive(Clone, Debug, PartialEq, Derivative)]
#[derivative(Hash(bound = "T: EthSpec"))]
pub struct AvailableBlock<T: EthSpec> {
    pub block: Arc<SignedBeaconBlock<T>>,
    pub blobs: VerifiedBlobs<T>,
}

impl<T: EthSpec> AvailableBlock<T> {
    pub fn blobs(&self) -> Option<Arc<BlobSidecarList<T>>> {
        match &self.blobs {
            VerifiedBlobs::EmptyBlobs | VerifiedBlobs::NotRequired | VerifiedBlobs::PreEip4844 => {
                None
            }
            VerifiedBlobs::Available(blobs) => Some(blobs.clone()),
        }
    }

    pub fn deconstruct(self) -> (Arc<SignedBeaconBlock<T>>, Option<Arc<BlobSidecarList<T>>>) {
        match self.blobs {
            VerifiedBlobs::EmptyBlobs | VerifiedBlobs::NotRequired | VerifiedBlobs::PreEip4844 => {
                (self.block, None)
            }
            VerifiedBlobs::Available(blobs) => (self.block, Some(blobs)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Derivative)]
#[derivative(Hash(bound = "E: EthSpec"))]
pub enum VerifiedBlobs<E: EthSpec> {
    /// These blobs are available.
    Available(Arc<BlobSidecarList<E>>),
    /// This block is from outside the data availability boundary so doesn't require
    /// a data availability check.
    NotRequired,
    /// The block's `kzg_commitments` field is empty so it does not contain any blobs.
    EmptyBlobs,
    /// This is a block prior to the 4844 fork, so doesn't require any blobs
    PreEip4844,
}

pub trait AsBlock<E: EthSpec> {
    fn slot(&self) -> Slot;
    fn epoch(&self) -> Epoch;
    fn parent_root(&self) -> Hash256;
    fn state_root(&self) -> Hash256;
    fn signed_block_header(&self) -> SignedBeaconBlockHeader;
    fn message(&self) -> BeaconBlockRef<E>;
    fn as_block(&self) -> &SignedBeaconBlock<E>;
    fn block_cloned(&self) -> Arc<SignedBeaconBlock<E>>;
    fn canonical_root(&self) -> Hash256;
}

#[derive(Debug, Clone, Derivative)]
#[derivative(Hash(bound = "E: EthSpec"))]
pub enum BlockWrapper<E: EthSpec> {
    /// This variant is fully available.
    /// i.e. for pre-4844 blocks, it contains a (`SignedBeaconBlock`, `Blobs::None`) and for
    /// post-4844 blocks, it contains a `SignedBeaconBlock` and a Blobs variant other than `Blobs::None`.
    Available(AvailableBlock<E>),
    /// This variant is not fully available and requires blobs to become fully available.
    AvailabilityPending(Arc<SignedBeaconBlock<E>>),
}

impl<E: EthSpec> BlockWrapper<E> {
    pub fn into_available_block(self) -> Option<AvailableBlock<E>> {
        match self {
            BlockWrapper::AvailabilityPending(_) => None,
            BlockWrapper::Available(block) => Some(block),
        }
    }
}

impl<E: EthSpec> AsBlock<E> for BlockWrapper<E> {
    fn slot(&self) -> Slot {
        match self {
            BlockWrapper::Available(block) => block.block.slot(),
            BlockWrapper::AvailabilityPending(block) => block.slot(),
        }
    }
    fn epoch(&self) -> Epoch {
        match self {
            BlockWrapper::Available(block) => block.block.epoch(),
            BlockWrapper::AvailabilityPending(block) => block.epoch(),
        }
    }
    fn parent_root(&self) -> Hash256 {
        match self {
            BlockWrapper::Available(block) => block.block.parent_root(),
            BlockWrapper::AvailabilityPending(block) => block.parent_root(),
        }
    }
    fn state_root(&self) -> Hash256 {
        match self {
            BlockWrapper::Available(block) => block.block.state_root(),
            BlockWrapper::AvailabilityPending(block) => block.state_root(),
        }
    }
    fn signed_block_header(&self) -> SignedBeaconBlockHeader {
        match &self {
            BlockWrapper::Available(block) => block.block.signed_block_header(),
            BlockWrapper::AvailabilityPending(block) => block.signed_block_header(),
        }
    }
    fn message(&self) -> BeaconBlockRef<E> {
        match &self {
            BlockWrapper::Available(block) => block.block.message(),
            BlockWrapper::AvailabilityPending(block) => block.message(),
        }
    }
    fn as_block(&self) -> &SignedBeaconBlock<E> {
        match &self {
            BlockWrapper::Available(block) => &block.block,
            BlockWrapper::AvailabilityPending(block) => &block,
        }
    }
    fn block_cloned(&self) -> Arc<SignedBeaconBlock<E>> {
        match &self {
            BlockWrapper::Available(block) => block.block.clone(),
            BlockWrapper::AvailabilityPending(block) => block.clone(),
        }
    }
    fn canonical_root(&self) -> Hash256 {
        match &self {
            BlockWrapper::Available(block) => block.block.canonical_root(),
            BlockWrapper::AvailabilityPending(block) => block.canonical_root(),
        }
    }
}

impl<E: EthSpec> AsBlock<E> for &BlockWrapper<E> {
    fn slot(&self) -> Slot {
        match self {
            BlockWrapper::Available(block) => block.block.slot(),
            BlockWrapper::AvailabilityPending(block) => block.slot(),
        }
    }
    fn epoch(&self) -> Epoch {
        match self {
            BlockWrapper::Available(block) => block.block.epoch(),
            BlockWrapper::AvailabilityPending(block) => block.epoch(),
        }
    }
    fn parent_root(&self) -> Hash256 {
        match self {
            BlockWrapper::Available(block) => block.block.parent_root(),
            BlockWrapper::AvailabilityPending(block) => block.parent_root(),
        }
    }
    fn state_root(&self) -> Hash256 {
        match self {
            BlockWrapper::Available(block) => block.block.state_root(),
            BlockWrapper::AvailabilityPending(block) => block.state_root(),
        }
    }
    fn signed_block_header(&self) -> SignedBeaconBlockHeader {
        match &self {
            BlockWrapper::Available(block) => block.block.signed_block_header(),
            BlockWrapper::AvailabilityPending(block) => block.signed_block_header(),
        }
    }
    fn message(&self) -> BeaconBlockRef<E> {
        match &self {
            BlockWrapper::Available(block) => block.block.message(),
            BlockWrapper::AvailabilityPending(block) => block.message(),
        }
    }
    fn as_block(&self) -> &SignedBeaconBlock<E> {
        match &self {
            BlockWrapper::Available(block) => &block.block,
            BlockWrapper::AvailabilityPending(block) => &block,
        }
    }
    fn block_cloned(&self) -> Arc<SignedBeaconBlock<E>> {
        match &self {
            BlockWrapper::Available(block) => block.block.clone(),
            BlockWrapper::AvailabilityPending(block) => block.clone(),
        }
    }
    fn canonical_root(&self) -> Hash256 {
        match &self {
            BlockWrapper::Available(block) => block.block.canonical_root(),
            BlockWrapper::AvailabilityPending(block) => block.canonical_root(),
        }
    }
}

impl<E: EthSpec> From<SignedBeaconBlock<E>> for BlockWrapper<E> {
    fn from(block: SignedBeaconBlock<E>) -> Self {
        BlockWrapper::AvailabilityPending(Arc::new(block))
    }
}

impl<E: EthSpec> From<Arc<SignedBeaconBlock<E>>> for BlockWrapper<E> {
    fn from(block: Arc<SignedBeaconBlock<E>>) -> Self {
        BlockWrapper::AvailabilityPending(block)
    }
}

impl<E: EthSpec> From<AvailableBlock<E>> for BlockWrapper<E> {
    fn from(block: AvailableBlock<E>) -> Self {
        BlockWrapper::Available(block)
    }
}
