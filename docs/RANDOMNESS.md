# Deterministic randomness contract

Word Arena V1 treats randomness as a replay input, not ambient platform state.
The engine never calls an operating-system or language-runtime random-number
generator. An application supplies exactly 32 seed bytes and retains them as a
private game secret until the configured post-game reveal policy permits
disclosure.

## Versioned algorithm

`xoshiro256-star-star-v1` is the complete V1 contract:

1. Hash `word-arena-xoshiro256-star-star-v1`, a zero byte, and the 32 seed
   bytes with SHA-256.
2. Read the digest as four consecutive big-endian `u64` state words. The
   all-zero fallback is defined defensively even though no known SHA-256 input
   reaches it.
3. Generate values with the published xoshiro256** transition and output
   function.
4. Map values to `[0, upper)` with rejection sampling. Modulo reduction is used
   only after values below `(-upper mod upper)` have been rejected, avoiding
   modulo bias.
5. Shuffle stable physical tile IDs once with descending Fisher-Yates. The bag
   draws from the end, dealing seat one and then seat two in stable rack order.
6. An exchange draws replacements before returned tiles re-enter the bag.
   Returned tiles are sorted by stable ID, appended to the remaining bag, and
   the whole bag is Fisher-Yates shuffled from a SHA-256-derived exchange state
   domain-separated by the private seed and committed transition version. This
   prevents caller-supplied tile order from changing the result and prevents an
   exchanged tile from being redrawn in the same action.

Any change to seed expansion, byte order, generator, bounded sampling, shuffle,
tile construction order, draw end, seat order, or exchange derivation requires
a new algorithm ID. Authoritative artifacts must record that ID.

## Commitment and reveal

Before live play, the application may publish the lowercase SHA-256 of
`word-arena-seed-commitment-v1`, a zero byte, the algorithm ID, a zero byte,
and the exact seed bytes. After the game, revealing the algorithm ID and seed
lets a verifier reproduce the bag and compare the commitment. A substituted
seed does not verify.

The commitment does not make a low-entropy seed safe. Applications must source
unpredictable 32-byte seeds and keep them secret during live play. The engine
intentionally provides no seed-generation policy.

## Information boundaries

The authoritative setup snapshot serializes the algorithm, commitment, exact
private bag order, and both racks so persistence round-trips without a reshuffle.
It does not serialize the seed. Public game state exposes neither the seed nor
the future bag order. Seat-specific rack projections and post-game reveal
artifacts belong to the application layer.

Players never receive a standalone draw action. Engine transitions draw only
after a legal placement or exchange, which prevents clients from sampling or
advancing the private bag independently.

## Verification

Engine tests pin one golden seed to its commitment, both opening racks, and a
SHA-256 fingerprint covering the complete remaining bag order. Property tests
exercise arbitrary seeds against the English and French distributions, exact
commitment verification, deterministic reproduction, unique tile identity, and
conservation. Private snapshot round trips and public no-leak serialization are
also tested.
