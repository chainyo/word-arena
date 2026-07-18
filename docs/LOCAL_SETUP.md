# Local setup and lexicon packs

`cargo xtask setup` is the canonical first-time setup command. It verifies Bun
and curl, installs the locked web dependencies with Bun, then downloads any
missing English and French Word Arena lexicon packs from the immutable URLs in
[`lexicons/registry.toml`](../lexicons/registry.toml).

The first run downloads about 1.2 MB of compiled pack data. A pack is never
made visible to the server until its compressed size and SHA-256, safe archive
layout, complete manifest, every manifest-listed file, content checksum,
license, source notice, third-party notices, and FST index have all passed
validation. Publication is a final directory rename from unique staging data.
An interruption, concurrent setup, or failed validation cannot replace an
already installed identity.

These are separately licensed **Word Arena lexicons**, not official SCRABBLE
tournament dictionaries and not claims of NWL, Collins, or ODS compatibility.

## Commands

```bash
# Install locked frontend dependencies and missing pinned packs.
cargo xtask setup

# Make no network request; verify installed packs or reinstall from verified cache.
cargo xtask setup --offline

# Inspect and manage the pinned immutable identities.
cargo xtask lexicon list
cargo xtask lexicon audit
cargo xtask lexicon inspect word-arena-fr-v1
cargo xtask lexicon verify
cargo xtask lexicon verify word-arena-en-world-v1
cargo xtask lexicon install word-arena-fr-v1
cargo xtask lexicon install word-arena-fr-v1 --offline
cargo xtask lexicon remove word-arena-fr-v1
```

`remove` first validates the exact installed identity, then moves it beneath the
local `trash/lexicons` directory and prints the recovery path. It does not erase
the downloaded artifact cache. Running `setup` again restores a missing pinned
identity from that verified cache without downloading it again.

After setup, the server validates and retains both complete offline indexes
before opening its HTTP listener:

```bash
cargo run -p word-arena-server
curl http://127.0.0.1:3000/health
```

Normal word lookup has no HTTP fallback. If either pack is missing, corrupt, or
ambiguous, server startup fails with a setup or identity-selection diagnostic.

## Data and cache locations

The default paths follow each operating system:

| System | Durable data | Download cache |
| --- | --- | --- |
| macOS | `~/Library/Application Support/Word Arena` | `~/Library/Caches/Word Arena` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/word-arena` | `${XDG_CACHE_HOME:-~/.cache}/word-arena` |
| Windows | `%LOCALAPPDATA%\Word Arena` | `%LOCALAPPDATA%\Word Arena\cache` |

Set `WORD_ARENA_DATA_DIR` to use an explicit self-contained location. With an
override, durable data is stored directly under that directory and the cache is
stored under its `cache/` child:

```bash
WORD_ARENA_DATA_DIR=/absolute/path/to/word-arena-data cargo xtask setup
```

Installed identities use this immutable layout:

```text
lexicons/<pack-id>/<pack-version>/<content-sha256>/
```

Downloaded archives use content-addressed cache names. Upstream archives,
compiled packs, caches, and build outputs are ignored by Git and must never be
committed.

## Updates and rollback

Pack releases and application rulesets are immutable pins. Updating the
repository may change the registry/ruleset to a newer pack version, but setup
installs that identity beside existing versions rather than replacing bytes in
place:

```bash
git pull --ff-only
cargo xtask setup
cargo xtask lexicon verify
```

Active games and replays continue to require the exact identity they recorded.
Keep every referenced version installed or available from its immutable release.
Never rename a different checksum into an old version directory.

To roll the application and default ruleset back, check out the prior reviewed
commit or release that contains the older registry, then run setup again. The
old immutable pack is reused when still installed/cached or downloaded from its
unchangeable release URL:

```bash
git switch --detach <prior-application-tag-or-commit>
cargo xtask setup
cargo xtask lexicon verify
```

Return to normal development with `git switch main`. Rollback does not mutate
an in-use pack or replay. Do not remove an older identity until no active game
or published replay references it and the release retention policy permits it.

## Reproducing artifacts from source

Release maintainers can rebuild one or both artifacts directly from the pinned
SCOWLv1 and Morphalou archives:

```bash
cargo xtask lexicon build --from-source --output /absolute/output/directory
cargo xtask lexicon build --from-source word-arena-en-world-v1 \
  --output /absolute/output/directory
```

Add `--release-materials` to also retain the exact upstream source archive,
deterministically compressed legible `keys.txt`, and row-level `audit.jsonl` for
each selected pack. The complete publishing procedure is documented in
[`lexicons/RELEASING.md`](../lexicons/RELEASING.md).

The command downloads the exact source archives recorded in
[`lexicons/sources.toml`](../lexicons/sources.toml), enforces their checksums and
the versioned filter/curation contracts, assembles deterministic archives, and
requires their content hash, compressed hash, and size to match the install
registry. Generated files must remain outside the repository.

`--allow-registry-mismatch` is reserved for bootstrapping an intentional new
pack release: it prints the newly built pins but does not modify the registry.
Review and commit registry changes separately.

## Recovery

- A checksum, manifest, index, or notice error means the downloaded artifact is
  unusable. Existing installed identities remain untouched. Retry online after
  the registry or release artifact is corrected.
- A network interruption leaves only temporary staging data, which is removed
  automatically. Retry when online, or use `--offline` when the verified cache
  is already present.
- An offline-cache error identifies the exact missing cache path. Run setup once
  with network access.
- An ambiguous installed-pack error means multiple immutable identities are
  present for a caller that did not supply an exact pin. Game creation, resume,
  replay, and production server startup load exact ruleset identities and never
  choose a substitute. Inspect the installed versions and retain each identity
  required by a game or replay.
