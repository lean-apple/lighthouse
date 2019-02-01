use crate::{
    beacon_state::{CommitteesError, EpochProcessingError},
    BeaconState, ChainSpec, Hash256,
};

#[derive(Debug, PartialEq)]
pub enum Error {
    CommitteesError(CommitteesError),
    EpochProcessingError(EpochProcessingError),
}

impl BeaconState {
    pub fn per_slot_processing(
        &mut self,
        previous_block_root: Hash256,
        spec: &ChainSpec,
    ) -> Result<(), Error> {
        if (self.slot + 1) % spec.epoch_length == 0 {
            self.per_epoch_processing(spec)?;
        }

        self.slot += 1;

        let block_proposer = self.get_beacon_proposer_index(self.slot, spec)?;

        self.validator_registry[block_proposer].proposer_slots += 1;
        self.latest_randao_mixes[(self.slot % spec.latest_randao_mixes_length) as usize] =
            self.latest_randao_mixes[((self.slot - 1) % spec.latest_randao_mixes_length) as usize];

        // Block roots.
        self.latest_block_roots[((self.slot - 1) % spec.latest_block_roots_length) as usize] =
            previous_block_root;

        if self.slot % spec.latest_block_roots_length == 0 {
            let root = merkle_root(&self.latest_block_roots[..]);
            self.batched_block_roots.push(root);
        }
        Ok(())
    }

    pub fn attestation_slot_and_shard_for_validator(
        &self,
        validator_index: usize,
        spec: &ChainSpec,
    ) -> Result<Option<(u64, u64, u64)>, CommitteesError> {
        let mut result = None;
        for slot in self.get_current_epoch_boundaries(spec.epoch_length) {
            for (committee, shard) in self.get_crosslink_committees_at_slot(slot, spec)? {
                if let Some(committee_index) = committee.iter().position(|&i| i == validator_index)
                {
                    result = Some((slot, shard, committee_index as u64));
                }
            }
        }
        Ok(result)
    }
}

fn merkle_root(_input: &[Hash256]) -> Hash256 {
    Hash256::zero()
}

impl From<CommitteesError> for Error {
    fn from(e: CommitteesError) -> Error {
        Error::CommitteesError(e)
    }
}

impl From<EpochProcessingError> for Error {
    fn from(e: EpochProcessingError) -> Error {
        Error::EpochProcessingError(e)
    }
}
