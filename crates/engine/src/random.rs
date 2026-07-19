//! Versioned deterministic randomness, private bag construction, and deals.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{Bag, PhysicalTile, Rack, Ruleset, Seat, TileId};

/// Independently versioned PRNG and shuffle contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RngAlgorithm {
    /// SHA-256-expanded 256-bit seed, xoshiro256** stream, rejection-sampled
    /// bounds, and descending Fisher-Yates shuffle.
    Xoshiro256StarStarV1,
}

impl RngAlgorithm {
    /// Stable identifier recorded with commitments and replay inputs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Xoshiro256StarStarV1 => "xoshiro256-star-star-v1",
        }
    }
}

/// Fixed 256-bit game seed supplied by an application boundary.
#[derive(Clone, Eq, PartialEq)]
pub struct GameSeed([u8; 32]);

impl fmt::Debug for GameSeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GameSeed([REDACTED])")
    }
}

impl GameSeed {
    /// Wraps exact seed bytes without consulting platform randomness.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Bytes revealed only after the live game policy permits it.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Builds the pre-game commitment for the selected RNG contract.
    #[must_use]
    pub fn commitment(&self, algorithm: RngAlgorithm) -> SeedCommitment {
        SeedCommitment {
            algorithm,
            sha256: seed_commitment_sha256(algorithm, self),
        }
    }
}

/// Public pre-game proof binding a future seed reveal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedCommitment {
    /// Exact algorithm the seed will drive.
    pub algorithm: RngAlgorithm,
    /// Lowercase domain-separated SHA-256.
    pub sha256: String,
}

impl SeedCommitment {
    /// Verifies one revealed seed against this exact commitment.
    #[must_use]
    pub fn verify(&self, seed: &GameSeed) -> bool {
        self.sha256 == seed_commitment_sha256(self.algorithm, seed)
    }
}

/// Authoritative private output of deterministic setup.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialDeal {
    algorithm: RngAlgorithm,
    commitment: SeedCommitment,
    bag: Bag,
    racks: [Rack; 2],
}

impl fmt::Debug for InitialDeal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InitialDeal")
            .field("algorithm", &self.algorithm)
            .field("commitment", &self.commitment)
            .field("bag", &"[REDACTED]")
            .field("racks", &"[REDACTED]")
            .finish()
    }
}

impl InitialDeal {
    /// Versioned algorithm used for every bag decision.
    #[must_use]
    pub const fn algorithm(&self) -> RngAlgorithm {
        self.algorithm
    }

    /// Public commitment safe to expose before the game.
    #[must_use]
    pub const fn commitment(&self) -> &SeedCommitment {
        &self.commitment
    }

    /// Current opening rack for one authenticated seat.
    #[must_use]
    pub fn rack(&self, seat: Seat) -> &Rack {
        &self.racks[seat.index()]
    }

    /// Public count of tiles remaining after the opening deal.
    #[must_use]
    pub const fn bag_len(&self) -> usize {
        self.bag.len()
    }

    /// Verifies the complete private setup against its ruleset distribution.
    ///
    /// # Errors
    ///
    /// Returns [`ConservationError`] for missing, duplicate, out-of-range, or
    /// face-substituted tiles.
    pub fn verify_conservation(&self, ruleset: &Ruleset) -> Result<(), ConservationError> {
        verify_tile_conservation(ruleset, &self.bag, &self.racks, &[])
    }

    pub(crate) fn into_parts(self) -> (Bag, [Rack; 2]) {
        (self.bag, self.racks)
    }
}

/// Creates stable physical tiles, shuffles once, and deals both opening racks.
///
/// The bag order and seed remain private in the returned authoritative value.
/// There is deliberately no player-facing draw operation.
///
/// # Errors
///
/// Returns [`RandomError`] when the ruleset is invalid or its distribution
/// cannot be represented by stable V1 tile IDs.
pub fn prepare_initial_deal(
    ruleset: &Ruleset,
    seed: &GameSeed,
) -> Result<InitialDeal, RandomError> {
    ruleset
        .validate()
        .map_err(|error| RandomError::Ruleset(error.to_string()))?;
    let algorithm = RngAlgorithm::Xoshiro256StarStarV1;
    let mut tiles = build_tiles(ruleset)?;
    DeterministicRng::new(seed).shuffle(&mut tiles);
    let mut bag = Bag::new(tiles);
    let mut racks = [Rack::default(), Rack::default()];
    for seat in Seat::ALL {
        racks[seat.index()].extend(bag.draw_up_to(usize::from(ruleset.game.rack_capacity)));
    }
    let deal = InitialDeal {
        algorithm,
        commitment: seed.commitment(algorithm),
        bag,
        racks,
    };
    deal.verify_conservation(ruleset)?;
    Ok(deal)
}

pub(crate) fn return_tiles_to_bag(
    bag: &mut Bag,
    mut returned: Vec<PhysicalTile>,
    seed: &GameSeed,
    transition: u64,
) {
    returned.sort_unstable_by_key(|tile| tile.id);
    let mut tiles = bag.tiles().to_vec();
    tiles.extend(returned);
    DeterministicRng::for_exchange(seed, transition).shuffle(&mut tiles);
    *bag = Bag::new(tiles);
}

/// Verifies exact physical ownership across every authoritative location.
///
/// # Errors
///
/// Returns [`ConservationError`] for a count, ID, or face mismatch.
pub fn verify_tile_conservation(
    ruleset: &Ruleset,
    bag: &Bag,
    racks: &[Rack; 2],
    board: &[PhysicalTile],
) -> Result<(), ConservationError> {
    let expected_total =
        usize::try_from(ruleset.game.total_tiles()).map_err(|_| ConservationError::Count {
            expected: usize::MAX,
            actual: 0,
        })?;
    let locations = bag
        .tiles()
        .iter()
        .chain(racks.iter().flat_map(Rack::tiles))
        .chain(board.iter());
    let expected_faces = ruleset
        .game
        .tiles
        .iter()
        .flat_map(|definition| std::iter::repeat_n(&definition.face, usize::from(definition.count)))
        .collect::<Vec<_>>();
    let mut ids = BTreeSet::new();
    let mut actual_total = 0_usize;
    for tile in locations {
        actual_total = actual_total
            .checked_add(1)
            .ok_or(ConservationError::Count {
                expected: expected_total,
                actual: usize::MAX,
            })?;
        let tile_index = usize::from(tile.id.0);
        if tile_index >= expected_total {
            return Err(ConservationError::TileIdOutOfRange {
                id: tile.id,
                tile_count: expected_total,
            });
        }
        if !ids.insert(tile.id) {
            return Err(ConservationError::DuplicateTileId { id: tile.id });
        }
        if expected_faces[tile_index] != &tile.face {
            return Err(ConservationError::FaceDistribution);
        }
    }
    if actual_total != expected_total {
        return Err(ConservationError::Count {
            expected: expected_total,
            actual: actual_total,
        });
    }
    Ok(())
}

/// Deterministic setup failure.
#[derive(Debug, Error)]
pub enum RandomError {
    /// Selected immutable ruleset failed validation.
    #[error("cannot prepare deterministic bag from ruleset: {0}")]
    Ruleset(String),
    /// Ruleset contains more tiles than V1 IDs can represent.
    #[error("ruleset contains too many physical tiles for V1 tile IDs")]
    TooManyTiles,
    /// Constructed setup violated conservation.
    #[error(transparent)]
    Conservation(#[from] ConservationError),
}

/// Physical tile conservation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ConservationError {
    /// Total tiles differ from the immutable distribution.
    #[error("tile count mismatch: expected {expected}, found {actual}")]
    Count {
        /// Ruleset total.
        expected: usize,
        /// Observed total.
        actual: usize,
    },
    /// The same stable tile appears in multiple locations.
    #[error("tile ID {id:?} appears more than once")]
    DuplicateTileId {
        /// Duplicate identity.
        id: TileId,
    },
    /// Tile identity cannot belong to the selected distribution.
    #[error("tile ID {id:?} is outside distribution size {tile_count}")]
    TileIdOutOfRange {
        /// Invalid identity.
        id: TileId,
        /// Valid exclusive upper bound.
        tile_count: usize,
    },
    /// Letter/blank counts differ despite matching total count.
    #[error("physical tile faces differ from the ruleset distribution")]
    FaceDistribution,
}

#[derive(Clone, Debug)]
struct DeterministicRng {
    state: [u64; 4],
}

impl DeterministicRng {
    fn new(seed: &GameSeed) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"word-arena-xoshiro256-star-star-v1\0");
        digest.update(seed.as_bytes());
        let bytes = digest.finalize();
        let mut state = [0_u64; 4];
        for (index, chunk) in bytes.chunks_exact(8).enumerate() {
            state[index] = u64::from_be_bytes(chunk.try_into().expect("eight-byte chunk"));
        }
        if state == [0; 4] {
            state[0] = 0x9e37_79b9_7f4a_7c15;
        }
        Self { state }
    }

    fn for_exchange(seed: &GameSeed, transition: u64) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"word-arena-exchange-xoshiro256-star-star-v1\0");
        digest.update(seed.as_bytes());
        digest.update(transition.to_be_bytes());
        let bytes = digest.finalize();
        let mut state = [0_u64; 4];
        for (index, chunk) in bytes.chunks_exact(8).enumerate() {
            state[index] = u64::from_be_bytes(chunk.try_into().expect("eight-byte chunk"));
        }
        if state == [0; 4] {
            state[0] = 0x9e37_79b9_7f4a_7c15;
        }
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let shifted = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= shifted;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }

    fn uniform_below(&mut self, upper: u64) -> u64 {
        debug_assert!(upper > 0);
        let threshold = upper.wrapping_neg() % upper;
        loop {
            let value = self.next_u64();
            if value >= threshold {
                return value % upper;
            }
        }
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for upper_index in (1..values.len()).rev() {
            let upper = u64::try_from(upper_index + 1).expect("slice length fits u64");
            let selected = usize::try_from(self.uniform_below(upper)).expect("index fits usize");
            values.swap(upper_index, selected);
        }
    }
}

fn build_tiles(ruleset: &Ruleset) -> Result<Vec<PhysicalTile>, RandomError> {
    let capacity =
        usize::try_from(ruleset.game.total_tiles()).map_err(|_| RandomError::TooManyTiles)?;
    let mut tiles = Vec::with_capacity(capacity);
    for definition in &ruleset.game.tiles {
        for _ in 0..definition.count {
            let id = u16::try_from(tiles.len()).map_err(|_| RandomError::TooManyTiles)?;
            tiles.push(PhysicalTile {
                id: TileId(id),
                face: definition.face.clone(),
            });
        }
    }
    Ok(tiles)
}

fn seed_commitment_sha256(algorithm: RngAlgorithm, seed: &GameSeed) -> String {
    let mut hash = Sha256::new();
    hash.update(b"word-arena-seed-commitment-v1\0");
    hash.update(algorithm.as_str().as_bytes());
    hash.update([0]);
    hash.update(seed.as_bytes());
    hex_lower(&hash.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use crate::{
        Bag, GameMode, GamePhase, PhysicalTile, PublicGameState, Rack, Ruleset, Score, Seat,
        TileFace, TileId, TileToken,
    };

    use super::{
        ConservationError, GameSeed, InitialDeal, RngAlgorithm, prepare_initial_deal,
        verify_tile_conservation,
    };

    #[test]
    fn draw_is_partial_and_empty_safe_without_replacement() {
        let mut bag = Bag::new(vec![tile(0), tile(1), tile(2)]);
        assert_eq!(ids(&bag.draw_up_to(2)), [2, 1]);
        assert_eq!(ids(&bag.draw_up_to(7)), [0]);
        assert!(bag.draw_up_to(1).is_empty());
        assert!(bag.is_empty());
    }

    #[test]
    fn golden_seed_pins_commitment_bag_order_and_opening_racks() {
        let ruleset = Ruleset::english_v1();
        let seed = GameSeed::from_bytes([0x42; 32]);
        let deal = prepare_initial_deal(&ruleset, &seed).unwrap();

        assert_eq!(deal.algorithm(), RngAlgorithm::Xoshiro256StarStarV1);
        assert_eq!(
            deal.commitment().sha256,
            "a35519a6a43fe0c018e2d210d97489c5eac3d9eaf13010117b608cc2b9a6957e"
        );
        assert_eq!(deal.bag_len(), 86);
        assert_eq!(
            ids(deal.rack(Seat::One).tiles()),
            [49, 73, 64, 41, 91, 79, 45]
        );
        assert_eq!(
            ids(deal.rack(Seat::Two).tiles()),
            [50, 0, 93, 54, 14, 15, 43]
        );
        assert_eq!(
            setup_fingerprint(&deal),
            "c9d730d6274a4c805705b7572486bfee89d78c85353b5fa5bafe234bb0a89976"
        );
    }

    #[test]
    fn exact_seed_reveal_verifies_and_substitution_fails() {
        let seed = GameSeed::from_bytes([11; 32]);
        assert_eq!(format!("{seed:?}"), "GameSeed([REDACTED])");
        let commitment = seed.commitment(RngAlgorithm::Xoshiro256StarStarV1);
        assert!(commitment.verify(&seed));
        assert!(!commitment.verify(&GameSeed::from_bytes([12; 32])));
    }

    #[test]
    fn private_setup_snapshot_round_trip_preserves_exact_order() {
        let deal =
            prepare_initial_deal(&Ruleset::french_v1(), &GameSeed::from_bytes([23; 32])).unwrap();
        let bytes = serde_json::to_vec(&deal).unwrap();
        let restored: InitialDeal = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, deal);
        assert_eq!(setup_fingerprint(&restored), setup_fingerprint(&deal));
    }

    #[test]
    fn built_in_distributions_deal_without_replacement() {
        for (ruleset, expected_bag) in [(Ruleset::english_v1(), 86), (Ruleset::french_v1(), 88)] {
            let deal = prepare_initial_deal(&ruleset, &GameSeed::from_bytes([31; 32])).unwrap();
            assert_eq!(deal.bag_len(), expected_bag);
            assert_eq!(deal.rack(Seat::One).len(), 7);
            assert_eq!(deal.rack(Seat::Two).len(), 7);
            deal.verify_conservation(&ruleset).unwrap();
        }
    }

    #[test]
    fn conservation_rejects_missing_duplicate_out_of_range_and_substituted_tiles() {
        let ruleset = Ruleset::english_v1();
        let deal = prepare_initial_deal(&ruleset, &GameSeed::from_bytes([9; 32])).unwrap();
        let racks = deal.racks.clone();

        let mut missing = deal.bag.tiles().to_vec();
        missing.pop();
        assert!(matches!(
            verify_tile_conservation(&ruleset, &Bag::new(missing), &racks, &[]),
            Err(ConservationError::Count { .. })
        ));

        let mut duplicate = deal.bag.tiles().to_vec();
        duplicate[1].id = duplicate[0].id;
        assert!(matches!(
            verify_tile_conservation(&ruleset, &Bag::new(duplicate), &racks, &[]),
            Err(ConservationError::DuplicateTileId { .. })
        ));

        let mut outside = deal.bag.tiles().to_vec();
        outside[0].id = TileId(100);
        assert!(matches!(
            verify_tile_conservation(&ruleset, &Bag::new(outside), &racks, &[]),
            Err(ConservationError::TileIdOutOfRange { .. })
        ));

        let mut substituted = deal.bag.tiles().to_vec();
        substituted[0].face = TileFace::Letter(TileToken::new("A").unwrap());
        if substituted[0].face == deal.bag.tiles()[0].face {
            substituted[0].face = TileFace::Letter(TileToken::new("Z").unwrap());
        }
        assert_eq!(
            verify_tile_conservation(&ruleset, &Bag::new(substituted), &racks, &[]),
            Err(ConservationError::FaceDistribution)
        );

        let mut swapped = deal.bag.tiles().to_vec();
        let other = swapped
            .iter()
            .position(|tile| tile.face != swapped[0].face)
            .unwrap();
        let first_face = swapped[0].face.clone();
        swapped[0].face = swapped[other].face.clone();
        swapped[other].face = first_face;
        assert_eq!(
            verify_tile_conservation(&ruleset, &Bag::new(swapped), &racks, &[]),
            Err(ConservationError::FaceDistribution)
        );
    }

    #[test]
    fn public_game_state_serialization_has_no_seed_or_future_bag() {
        let ruleset = Ruleset::english_v1();
        let state = PublicGameState {
            game_id: "public-contract".to_owned(),
            ruleset_id: ruleset.id,
            ruleset: ruleset.identity(),
            lexicon: ruleset.lexicon,
            mode: GameMode::Competitive,
            rng_algorithm: RngAlgorithm::Xoshiro256StarStarV1,
            seed_commitment: GameSeed::from_bytes([0; 32])
                .commitment(RngAlgorithm::Xoshiro256StarStarV1),
            board: vec![None; 225],
            scores: [Score::ZERO, Score::ZERO],
            current_player: Seat::One,
            version: 0,
            scoreless_turns: 0,
            rack_counts: [7, 7],
            bag_count: 86,
            phase: GamePhase::Active,
            result: None,
        };
        let value = serde_json::to_value(state).unwrap();
        let object = value.as_object().unwrap();
        for forbidden in ["seed", "bag", "future_tiles", "bag_order"] {
            assert!(!object.contains_key(forbidden));
        }
        assert_eq!(value["scores"], json!([0, 0]));
    }

    proptest! {
        #[test]
        fn deals_are_deterministic_and_conserve_both_distributions(seed in any::<[u8; 32]>()) {
            for ruleset in [Ruleset::english_v1(), Ruleset::french_v1()] {
                let game_seed = GameSeed::from_bytes(seed);
                let first = prepare_initial_deal(&ruleset, &game_seed).unwrap();
                let second = prepare_initial_deal(&ruleset, &game_seed).unwrap();
                prop_assert_eq!(&first, &second);
                prop_assert!(first.verify_conservation(&ruleset).is_ok());

                let mut all_ids = first.bag.tiles().iter()
                    .chain(first.racks.iter().flat_map(Rack::tiles))
                    .map(|tile| tile.id)
                    .collect::<Vec<_>>();
                all_ids.sort_unstable();
                prop_assert_eq!(all_ids.len(), ruleset.game.total_tiles() as usize);
                prop_assert!(all_ids.windows(2).all(|pair| pair[0] != pair[1]));
            }
        }

        #[test]
        fn commitments_bind_every_seed_bit(seed in any::<[u8; 32]>(), bit in 0_usize..256) {
            let exact = GameSeed::from_bytes(seed);
            let commitment = exact.commitment(RngAlgorithm::Xoshiro256StarStarV1);
            let mut changed = seed;
            changed[bit / 8] ^= 1 << (bit % 8);
            prop_assert!(commitment.verify(&exact));
            prop_assert!(!commitment.verify(&GameSeed::from_bytes(changed)));
        }
    }

    fn tile(id: u16) -> PhysicalTile {
        PhysicalTile {
            id: TileId(id),
            face: TileFace::Blank,
        }
    }

    fn ids(tiles: &[PhysicalTile]) -> Vec<u16> {
        tiles.iter().map(|tile| tile.id.0).collect()
    }

    fn setup_fingerprint(deal: &InitialDeal) -> String {
        let mut hash = Sha256::new();
        hash.update(deal.algorithm.as_str().as_bytes());
        hash.update([0]);
        for tile in deal.bag.tiles() {
            hash.update(tile.id.0.to_be_bytes());
        }
        for rack in &deal.racks {
            hash.update([0xff, 0xff]);
            for tile in rack.tiles() {
                hash.update(tile.id.0.to_be_bytes());
            }
        }
        super::hex_lower(&hash.finalize())
    }
}
