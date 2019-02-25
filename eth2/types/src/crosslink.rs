use crate::test_utils::TestRandom;
use crate::{Epoch, Hash256};
use rand::RngCore;
use serde_derive::Serialize;
use ssz_derive::{Decode, Encode, TreeHash};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Hash, Encode, Decode, TreeHash)]
pub struct Crosslink {
    pub epoch: Epoch,
    pub shard_block_root: Hash256,
}

impl Crosslink {
    /// Generates a new instance where `dynasty` and `hash` are both zero.
    pub fn zero() -> Self {
        Self {
            epoch: Epoch::new(0),
            shard_block_root: Hash256::zero(),
        }
    }
}

impl<T: RngCore> TestRandom<T> for Crosslink {
    fn random_for_test(rng: &mut T) -> Self {
        Self {
            epoch: <_>::random_for_test(rng),
            shard_block_root: <_>::random_for_test(rng),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{SeedableRng, TestRandom, XorShiftRng};
    use ssz::{ssz_encode, Decodable, TreeHash};

    #[test]
    pub fn test_ssz_round_trip() {
        let mut rng = XorShiftRng::from_seed([42; 16]);
        let original = Crosslink::random_for_test(&mut rng);

        let bytes = ssz_encode(&original);
        let (decoded, _) = <_>::ssz_decode(&bytes, 0).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    pub fn test_hash_tree_root_internal() {
        let mut rng = XorShiftRng::from_seed([42; 16]);
        let original = Crosslink::random_for_test(&mut rng);

        let result = original.hash_tree_root_internal();

        assert_eq!(result.len(), 32);
        // TODO: Add further tests
        // https://github.com/sigp/lighthouse/issues/170
    }
}
