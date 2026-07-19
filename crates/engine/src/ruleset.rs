use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use word_arena_lexicon::{
    CompatibilityContext, ENGLISH_NORMALIZATION_PROFILE, FRENCH_NORMALIZATION_PROFILE,
    NORMALIZATION_ALGORITHM, NORMALIZATION_VERSION, NormalizationDescriptor, PackIdentity,
    ensure_exact_pack,
};

use crate::{
    BoardDefinition, BoardSquare, Coordinate, GameError, Language, Premium, TileFace, TileToken,
    TileTokenError,
};

/// Static ruleset schema recorded with games and replay bundles.
pub const RULESET_SCHEMA_VERSION: u32 = 2;

const ENGLISH_CONTENT_SHA256: &str =
    "27faaa6b78de526d7e7681bf1af45ce952cb0400897190c79eab7c67b278a54b";
const FRENCH_CONTENT_SHA256: &str =
    "c926a5f1ead63711d041277c9bfb3af23f3a460bb6edf57ff66408552c495193";

/// Stable identifier for one immutable game-rules generation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RulesetId {
    /// English V1 using the curated world-English pack.
    EnglishV1,
    /// French V1 using the curated Morphalou-derived pack.
    FrenchV1,
}

impl RulesetId {
    /// Stable machine identifier included in ruleset hashes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnglishV1 => "english-v1",
            Self::FrenchV1 => "french-v1",
        }
    }
}

/// One face, quantity, and value in a physical tile distribution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TileDefinition {
    /// Printed letter or blank face.
    pub face: TileFace,
    /// Number of physically distinct tiles with this face.
    pub count: u16,
    /// Base point value. Blanks must be zero.
    pub value: u16,
}

/// Complete physical and scoring configuration used by one game.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameRules {
    /// Immutable row-major board and premium layout.
    pub board: BoardDefinition,
    /// Maximum number of tiles held by a full rack.
    pub rack_capacity: u8,
    /// Bonus for placing a full rack in one move.
    pub bingo_bonus: u16,
    /// Minimum bag size required before an exchange.
    pub exchange_minimum: u16,
    /// Consecutive scoreless turns that finish a game.
    pub scoreless_turn_limit: u8,
    /// Canonically ordered `A` through `Z` definitions followed by blank.
    pub tiles: Vec<TileDefinition>,
}

impl GameRules {
    /// Total number of physical tiles.
    #[must_use]
    pub fn total_tiles(&self) -> u32 {
        self.tiles.iter().map(|tile| u32::from(tile.count)).sum()
    }

    /// Returns the definition for one canonical letter token.
    #[must_use]
    pub fn letter(&self, token: &str) -> Option<&TileDefinition> {
        self.tiles.iter().find(|definition| {
            matches!(&definition.face, TileFace::Letter(letter) if letter.as_str() == token)
        })
    }

    /// Returns the blank definition.
    #[must_use]
    pub fn blank(&self) -> Option<&TileDefinition> {
        self.tiles
            .iter()
            .find(|definition| definition.face == TileFace::Blank)
    }
}

/// Immutable, externally recordable ruleset identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RulesetIdentity {
    /// Ruleset schema included in the digest.
    pub schema_version: u32,
    /// Human-stable ruleset name.
    pub ruleset_id: RulesetId,
    /// Lowercase SHA-256 over the canonical ruleset encoding.
    pub content_sha256: String,
}

/// Versioned rules and exact lexicon identity required for a new game.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ruleset {
    /// Ruleset schema version.
    pub schema_version: u32,
    /// Stable ruleset identity.
    pub id: RulesetId,
    /// Rules language.
    pub language: Language,
    /// Exact immutable pack allowed by this ruleset.
    pub lexicon: PackIdentity,
    /// Complete physical and scoring definition.
    pub game: GameRules,
}

impl Ruleset {
    /// English V1 production ruleset and release-pack pin.
    ///
    /// # Panics
    ///
    /// Panics only when the compile-time English or shared board fixture is
    /// malformed. CI and `cargo xtask ruleset verify` validate both fixtures.
    #[must_use]
    pub fn english_v1() -> Self {
        static RULESET: OnceLock<Ruleset> = OnceLock::new();
        RULESET
            .get_or_init(|| Self::verify_builtin(RulesetId::EnglishV1).expect("English V1 fixture"))
            .clone()
    }

    /// French V1 production ruleset and release-pack pin.
    ///
    /// # Panics
    ///
    /// Panics only when the compile-time French or shared board fixture is
    /// malformed. CI and `cargo xtask ruleset verify` validate both fixtures.
    #[must_use]
    pub fn french_v1() -> Self {
        static RULESET: OnceLock<Ruleset> = OnceLock::new();
        RULESET
            .get_or_init(|| Self::verify_builtin(RulesetId::FrenchV1).expect("French V1 fixture"))
            .clone()
    }

    /// Parses and structurally validates one committed static fixture.
    ///
    /// # Errors
    ///
    /// Returns [`RulesetFixtureError`] for unknown fields, malformed TOML,
    /// mismatched IDs/pins, invalid tokens, unsafe premium coordinates, or a
    /// structurally invalid expanded definition.
    pub fn verify_builtin(id: RulesetId) -> Result<Self, RulesetFixtureError> {
        let encoded = match id {
            RulesetId::EnglishV1 => include_str!("../../../rulesets/english-v1.toml"),
            RulesetId::FrenchV1 => include_str!("../../../rulesets/french-v1.toml"),
        };
        load_builtin_ruleset(
            encoded,
            include_str!("../../../rulesets/classic-board-v1.toml"),
            id,
        )
    }

    /// Returns the curated offline V1 ruleset for a supported language.
    ///
    /// # Errors
    ///
    /// Returns [`GameError::RulesetUnavailable`] until German or Spanish has a
    /// separately reviewed offline pack and immutable pin.
    pub fn for_language(language: Language) -> Result<Self, GameError> {
        match language {
            Language::English => Ok(Self::english_v1()),
            Language::French => Ok(Self::french_v1()),
            Language::German | Language::Spanish => Err(GameError::RulesetUnavailable { language }),
        }
    }

    /// Verifies structural invariants independently from the built-in pin.
    ///
    /// # Errors
    ///
    /// Returns [`RulesetDefinitionError`] for malformed board geometry,
    /// premiums, tile distributions, or gameplay limits.
    pub fn validate_definition(&self) -> Result<(), RulesetDefinitionError> {
        if self.schema_version != RULESET_SCHEMA_VERSION {
            return Err(RulesetDefinitionError::Schema {
                found: self.schema_version,
                expected: RULESET_SCHEMA_VERSION,
            });
        }
        validate_board(&self.game.board)?;
        validate_limits(&self.game)?;
        validate_tiles(&self.game.tiles, self.game.rack_capacity)?;
        Ok(())
    }

    /// Verifies structure and every field against the immutable built-in
    /// definition.
    ///
    /// # Errors
    ///
    /// Returns a structural error or [`GameError::InvalidRuleset`] when any
    /// immutable value differs.
    pub fn validate(&self) -> Result<(), GameError> {
        self.validate_definition()
            .map_err(|error| GameError::InvalidRulesetDefinition {
                ruleset: self.id,
                reason: error.to_string(),
            })?;
        let expected = match self.id {
            RulesetId::EnglishV1 => Self::english_v1(),
            RulesetId::FrenchV1 => Self::french_v1(),
        };
        if self == &expected {
            Ok(())
        } else {
            Err(GameError::InvalidRuleset { ruleset: self.id })
        }
    }

    /// Canonical content identity covering physical rules and exact lexicon.
    #[must_use]
    pub fn identity(&self) -> RulesetIdentity {
        RulesetIdentity {
            schema_version: self.schema_version,
            ruleset_id: self.id,
            content_sha256: ruleset_sha256(self),
        }
    }

    /// Base value for one canonical `A` through `Z` token.
    #[must_use]
    pub fn letter_value(&self, token: &str) -> Option<u16> {
        self.game.letter(token).map(|definition| definition.value)
    }

    pub(crate) fn ensure_lexicon(
        &self,
        context: CompatibilityContext,
        actual: &PackIdentity,
    ) -> Result<(), GameError> {
        self.validate()?;
        ensure_exact_pack(context, &self.lexicon, actual)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesetFixture {
    schema_version: u32,
    id: RulesetId,
    language: Language,
    pack_id: String,
    pack_version: String,
    pack_format_version: u32,
    locale: String,
    normalization_profile: String,
    lexicon_content_sha256: String,
    game: GameFixture,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GameFixture {
    rack_capacity: u8,
    bingo_bonus: u16,
    exchange_minimum: u16,
    scoreless_turn_limit: u8,
    tiles: Vec<TileFixture>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TileFixture {
    token: Option<String>,
    count: u16,
    value: u16,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardFixture {
    schema_version: u32,
    width: u8,
    height: u8,
    double_letter: Vec<[u8; 2]>,
    triple_letter: Vec<[u8; 2]>,
    double_word: Vec<[u8; 2]>,
    triple_word: Vec<[u8; 2]>,
}

fn load_builtin_ruleset(
    encoded: &str,
    board_encoded: &str,
    expected_id: RulesetId,
) -> Result<Ruleset, RulesetFixtureError> {
    let fixture: RulesetFixture = toml::from_str(encoded)?;
    if fixture.id != expected_id {
        return Err(RulesetFixtureError::Pin {
            reason: "fixture ID differs from selected built-in ruleset",
        });
    }
    let expected_lexicon = match fixture.id {
        RulesetId::EnglishV1 => (
            Language::English,
            ENGLISH_NORMALIZATION_PROFILE,
            ENGLISH_CONTENT_SHA256,
        ),
        RulesetId::FrenchV1 => (
            Language::French,
            FRENCH_NORMALIZATION_PROFILE,
            FRENCH_CONTENT_SHA256,
        ),
    };
    if fixture.language != expected_lexicon.0
        || fixture.normalization_profile != expected_lexicon.1
        || fixture.lexicon_content_sha256 != expected_lexicon.2
    {
        return Err(RulesetFixtureError::Pin {
            reason: "fixture language, normalization, or lexicon pin differs from V1",
        });
    }

    let board_fixture: BoardFixture = toml::from_str(board_encoded)?;
    if board_fixture.schema_version != 1 {
        return Err(RulesetFixtureError::Board {
            reason: "unsupported board fixture schema",
        });
    }

    let ruleset = Ruleset {
        schema_version: fixture.schema_version,
        id: fixture.id,
        language: fixture.language,
        lexicon: PackIdentity {
            pack_id: fixture.pack_id,
            pack_version: fixture.pack_version,
            format_version: fixture.pack_format_version,
            locale: fixture.locale,
            normalization: NormalizationDescriptor {
                algorithm: NORMALIZATION_ALGORITHM.to_owned(),
                version: NORMALIZATION_VERSION,
                profile: fixture.normalization_profile,
            },
            content_sha256: fixture.lexicon_content_sha256,
        },
        game: GameRules {
            board: expand_board(board_fixture)?,
            rack_capacity: fixture.game.rack_capacity,
            bingo_bonus: fixture.game.bingo_bonus,
            exchange_minimum: fixture.game.exchange_minimum,
            scoreless_turn_limit: fixture.game.scoreless_turn_limit,
            tiles: fixture
                .game
                .tiles
                .into_iter()
                .map(|tile| {
                    Ok(TileDefinition {
                        face: match tile.token {
                            Some(token) => TileFace::Letter(TileToken::new(token)?),
                            None => TileFace::Blank,
                        },
                        count: tile.count,
                        value: tile.value,
                    })
                })
                .collect::<Result<Vec<_>, RulesetFixtureError>>()?,
        },
    };
    ruleset.validate_definition()?;
    Ok(ruleset)
}

fn expand_board(fixture: BoardFixture) -> Result<BoardDefinition, RulesetFixtureError> {
    let square_count = usize::from(fixture.width)
        .checked_mul(usize::from(fixture.height))
        .ok_or(RulesetFixtureError::Board {
            reason: "board dimensions overflow",
        })?;
    let mut squares = Vec::with_capacity(square_count);
    for row in 0..fixture.height {
        for column in 0..fixture.width {
            squares.push(BoardSquare {
                coordinate: Coordinate::new(row, column),
                premium: Premium::Normal,
            });
        }
    }
    let mut board = BoardDefinition {
        width: fixture.width,
        height: fixture.height,
        squares,
    };
    for (premium, coordinates) in [
        (Premium::DoubleLetter, fixture.double_letter),
        (Premium::TripleLetter, fixture.triple_letter),
        (Premium::DoubleWord, fixture.double_word),
        (Premium::TripleWord, fixture.triple_word),
    ] {
        for [row, column] in coordinates {
            let coordinate = Coordinate::new(row, column);
            if !coordinate.in_bounds(board.width, board.height) {
                return Err(RulesetFixtureError::Board {
                    reason: "premium coordinate is out of bounds",
                });
            }
            let square = board.squares.get_mut(coordinate.index(board.width)).ok_or(
                RulesetFixtureError::Board {
                    reason: "premium coordinate is missing",
                },
            )?;
            if square.coordinate != coordinate || square.premium != Premium::Normal {
                return Err(RulesetFixtureError::Board {
                    reason: "premium coordinates overlap or are not canonical",
                });
            }
            square.premium = premium;
        }
    }
    Ok(board)
}

/// Static TOML fixture parsing or expansion failure.
#[derive(Debug, Error)]
pub enum RulesetFixtureError {
    /// TOML syntax, type, or unknown-field error.
    #[error("invalid ruleset fixture TOML: {0}")]
    Toml(#[from] toml::de::Error),
    /// Fixture does not match its selected immutable V1 identity.
    #[error("invalid ruleset fixture pin: {reason}")]
    Pin {
        /// Stable diagnostic.
        reason: &'static str,
    },
    /// Board fixture cannot expand safely.
    #[error("invalid board fixture: {reason}")]
    Board {
        /// Stable diagnostic.
        reason: &'static str,
    },
    /// Physical token is not one canonical A-Z letter.
    #[error(transparent)]
    TileToken(#[from] TileTokenError),
    /// Expanded ruleset violates schema invariants.
    #[error(transparent)]
    Definition(#[from] RulesetDefinitionError),
}

/// Structural ruleset validation failure.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RulesetDefinitionError {
    /// Unsupported schema.
    #[error("ruleset schema {found} is unsupported; expected {expected}")]
    Schema {
        /// Found version.
        found: u32,
        /// Supported version.
        expected: u32,
    },
    /// Invalid board dimensions or square enumeration.
    #[error("invalid board definition: {reason}")]
    Board {
        /// Stable diagnostic.
        reason: &'static str,
    },
    /// Premium layout is not mirror-symmetric.
    #[error("premium at {coordinate} is not horizontally and vertically symmetric")]
    PremiumAsymmetry {
        /// First asymmetric square.
        coordinate: Coordinate,
    },
    /// Invalid rack, exchange, bingo, or scoreless-turn parameter.
    #[error("invalid game limit: {reason}")]
    Limit {
        /// Stable diagnostic.
        reason: &'static str,
    },
    /// Invalid, incomplete, duplicate, or unordered tile definitions.
    #[error("invalid tile distribution: {reason}")]
    Tiles {
        /// Stable diagnostic.
        reason: &'static str,
    },
}

fn validate_board(board: &BoardDefinition) -> Result<(), RulesetDefinitionError> {
    if board.width == 0 || board.height == 0 || board.width != board.height {
        return board_error("board must be nonempty and square");
    }
    let expected = usize::from(board.width)
        .checked_mul(usize::from(board.height))
        .ok_or(RulesetDefinitionError::Board {
            reason: "board square count overflows",
        })?;
    if board.squares.len() != expected {
        return board_error("board must enumerate every square exactly once");
    }
    for (index, square) in board.squares.iter().enumerate() {
        let row = u8::try_from(index / usize::from(board.width)).map_err(|_| {
            RulesetDefinitionError::Board {
                reason: "board row cannot be represented",
            }
        })?;
        let column = u8::try_from(index % usize::from(board.width)).map_err(|_| {
            RulesetDefinitionError::Board {
                reason: "board column cannot be represented",
            }
        })?;
        if square.coordinate != Coordinate::new(row, column) {
            return board_error("squares must be unique and row-major");
        }
        let horizontal = Coordinate::new(row, board.width - 1 - column);
        let vertical = Coordinate::new(board.height - 1 - row, column);
        if board.square(horizontal).map(|value| value.premium) != Some(square.premium)
            || board.square(vertical).map(|value| value.premium) != Some(square.premium)
        {
            return Err(RulesetDefinitionError::PremiumAsymmetry {
                coordinate: square.coordinate,
            });
        }
    }
    let center = Coordinate::new(board.height / 2, board.width / 2);
    if board.square(center).map(|square| square.premium) != Some(Premium::DoubleWord) {
        return board_error("center square must be double-word");
    }
    Ok(())
}

fn validate_limits(game: &GameRules) -> Result<(), RulesetDefinitionError> {
    if game.rack_capacity == 0 {
        return limit_error("rack capacity must be positive");
    }
    if game.bingo_bonus == 0 {
        return limit_error("bingo bonus must be positive");
    }
    if game.exchange_minimum < u16::from(game.rack_capacity) {
        return limit_error("exchange minimum cannot be below rack capacity");
    }
    if game.scoreless_turn_limit == 0 {
        return limit_error("scoreless-turn limit must be positive");
    }
    Ok(())
}

fn validate_tiles(
    tiles: &[TileDefinition],
    rack_capacity: u8,
) -> Result<(), RulesetDefinitionError> {
    if tiles.len() != 27 {
        return tile_error("distribution must contain A-Z and one blank definition");
    }
    let mut total = 0_u16;
    let mut maximum_value = 0_u16;
    for (index, definition) in tiles.iter().enumerate() {
        if definition.count == 0 {
            return tile_error("every face must have a positive count");
        }
        total = total
            .checked_add(definition.count)
            .ok_or(RulesetDefinitionError::Tiles {
                reason: "tile count overflows",
            })?;
        maximum_value = maximum_value.max(definition.value);
        if index < 26 {
            let expected = char::from(b'A' + u8::try_from(index).expect("index is below 26"));
            let TileFace::Letter(token) = &definition.face else {
                return tile_error("letter definitions must precede blank");
            };
            if token.as_str() != expected.encode_utf8(&mut [0; 4]) || definition.value == 0 {
                return tile_error("letters must be ordered A-Z with positive values");
            }
        } else if definition.face != TileFace::Blank || definition.value != 0 {
            return tile_error("final definition must be a zero-value blank");
        }
    }
    if total < u16::from(rack_capacity) * 2 {
        return tile_error("distribution cannot deal two full opening racks");
    }
    u32::from(total)
        .checked_mul(u32::from(maximum_value))
        .ok_or(RulesetDefinitionError::Tiles {
            reason: "maximum tile-value total overflows",
        })?;
    Ok(())
}

fn ruleset_sha256(ruleset: &Ruleset) -> String {
    let mut hash = Sha256::new();
    hash.update(b"word-arena-ruleset-v1\0");
    hash.update(ruleset.schema_version.to_be_bytes());
    hash_string(&mut hash, ruleset.id.as_str());
    hash_string(&mut hash, ruleset.language.code());
    hash_pack(&mut hash, &ruleset.lexicon);
    hash.update([ruleset.game.board.width, ruleset.game.board.height]);
    hash.update(
        u32::try_from(ruleset.game.board.squares.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for square in &ruleset.game.board.squares {
        hash.update([
            square.coordinate.row,
            square.coordinate.column,
            premium_code(square.premium),
        ]);
    }
    hash.update([ruleset.game.rack_capacity]);
    hash.update(ruleset.game.bingo_bonus.to_be_bytes());
    hash.update(ruleset.game.exchange_minimum.to_be_bytes());
    hash.update([ruleset.game.scoreless_turn_limit]);
    hash.update(
        u32::try_from(ruleset.game.tiles.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for tile in &ruleset.game.tiles {
        match &tile.face {
            TileFace::Letter(token) => {
                hash.update([1]);
                hash_string(&mut hash, token.as_str());
            }
            TileFace::Blank => hash.update([0]),
        }
        hash.update(tile.count.to_be_bytes());
        hash.update(tile.value.to_be_bytes());
    }
    hex_lower(&hash.finalize())
}

fn hash_pack(hash: &mut Sha256, pack: &PackIdentity) {
    hash_string(hash, &pack.pack_id);
    hash_string(hash, &pack.pack_version);
    hash.update(pack.format_version.to_be_bytes());
    hash_string(hash, &pack.locale);
    hash_string(hash, &pack.normalization.algorithm);
    hash.update(pack.normalization.version.to_be_bytes());
    hash_string(hash, &pack.normalization.profile);
    hash_string(hash, &pack.content_sha256);
}

fn hash_string(hash: &mut Sha256, value: &str) {
    hash.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hash.update(value.as_bytes());
}

const fn premium_code(premium: Premium) -> u8 {
    match premium {
        Premium::Normal => 0,
        Premium::DoubleLetter => 1,
        Premium::TripleLetter => 2,
        Premium::DoubleWord => 3,
        Premium::TripleWord => 4,
    }
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

fn board_error<T>(reason: &'static str) -> Result<T, RulesetDefinitionError> {
    Err(RulesetDefinitionError::Board { reason })
}

fn limit_error<T>(reason: &'static str) -> Result<T, RulesetDefinitionError> {
    Err(RulesetDefinitionError::Limit { reason })
}

fn tile_error<T>(reason: &'static str) -> Result<T, RulesetDefinitionError> {
    Err(RulesetDefinitionError::Tiles { reason })
}

#[cfg(test)]
mod tests {
    use super::{RulesetFixtureError, RulesetId, load_builtin_ruleset};

    const ENGLISH: &str = include_str!("../../../rulesets/english-v1.toml");
    const BOARD: &str = include_str!("../../../rulesets/classic-board-v1.toml");

    #[test]
    fn static_fixture_loader_rejects_unknown_fields_and_pin_drift() {
        let unknown = format!("{ENGLISH}\nunknown = true\n");
        assert!(matches!(
            load_builtin_ruleset(&unknown, BOARD, RulesetId::EnglishV1),
            Err(RulesetFixtureError::Toml(_))
        ));

        let wrong_id = ENGLISH.replace("id = \"english-v1\"", "id = \"french-v1\"");
        assert!(matches!(
            load_builtin_ruleset(&wrong_id, BOARD, RulesetId::EnglishV1),
            Err(RulesetFixtureError::Pin { .. })
        ));
    }

    #[test]
    fn static_fixture_loader_rejects_tokens_and_overlapping_premiums() {
        let accented = ENGLISH.replacen("token = \"A\"", "token = \"É\"", 1);
        assert!(matches!(
            load_builtin_ruleset(&accented, BOARD, RulesetId::EnglishV1),
            Err(RulesetFixtureError::TileToken(_))
        ));

        let overlap = BOARD.replacen("[0, 3]", "[0, 0]", 1);
        assert!(matches!(
            load_builtin_ruleset(ENGLISH, &overlap, RulesetId::EnglishV1),
            Err(RulesetFixtureError::Board { .. })
        ));
    }
}
