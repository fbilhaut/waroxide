# orcrust

A Rust port of the native tools of [Wargus](https://github.com/Wargus/wargus),
the Warcraft II mod for the [Stratagus](https://github.com/Wargus/stratagus)
engine:

| Binary       | Replaces                    | Does                                                        |
|--------------|-----------------------------|-------------------------------------------------------------|
| `wartool`    | `wartool.cpp`               | Extracts the original CD data into PNG/WAV/MIDI/Lua files   |
| `pudconvert` | `pudconvert.cpp`, `pud.cpp` | Converts a `.pud` map to Stratagus `.smp.gz` / `.sms.gz`    |
| `wargus`     | `wargus.cpp` + launcher     | Extracts on first run, then starts Stratagus on the data    |

The game itself still runs on the Stratagus engine with the Wargus Lua
scripts. The extracted data is meant to match wartool 3.3.3's layout and
formats exactly, so data from either tool works with either launcher (see
"Checking against the original" below).

## macOS app

`macos/build-app.sh` builds a self-contained `Wargus.app` and `Wargus.dmg`
(Apple Silicon) in `target/macos/`: the launcher and extractor, the
Stratagus engine with its libraries, and the Wargus scripts. Players drag it
to Applications, open it, pick their CD (`REZDAT.WAR`) or CD image
(`.cue`/`.bin`/`.iso`), and play. Nothing else needs to be installed; the
soundtrack is ripped from an image and encoded to Ogg Vorbis by `wartool`
itself. The videos are only converted when `ffmpeg` (with libtheora) is
installed, and are skipped otherwise.

Building it needs Rust, the Xcode command line tools and
`brew install cmake sdl2_mixer sdl2_image dylibbundler`. Stratagus and
Wargus are fetched at pinned commits.

LAN multiplayer is Stratagus' own (UDP port 6660, hosts found automatically
over mDNS or joined by IP). All players need the same engine and scripts, so
everyone should use the same app build; their game data can come from
different CDs. On first use macOS asks for Local Network access (and, with
the firewall on, to accept incoming connections on the host).
`macos/test-multiplayer.sh <data-dir>` checks discovery and a 2-player game
between two instances on one Mac.

The app is ad-hoc signed, not notarized: on another Mac, open it the first
time with right-click → Open. The bundled Homebrew libraries require the
macOS version they were built on (shown at the end of the build).

## Building

```sh
cargo build --release            # native dialogs in the launcher (rfd)
cargo build --release --no-default-features   # console-only launcher
```

The launcher's install paths are set at build time, as in the CMake build:

- `WARGUS_DATA_PATH`: shared data directory (default `/usr/local/share/games/stratagus/wargus`)
- `WARGUS_SCRIPTS_PATH`: directory with the Wargus `scripts/`, `maps/`, `campaigns/`, `shaders/`, `contrib/` (defaults to the data path)
- `STRATAGUS_BIN`: the engine (default `stratagus`, looked up on `PATH`)

At runtime, `WARGUS_SCRIPTS_PATH` can also point to a Wargus checkout.

## Usage

```sh
# Extract from the DOS CD (its root or its DATA directory)...
wartool [-e|-n] [-v] [-r] /Volumes/WAR2 ~/Library/Application\ Support/Stratagus/data.Wargus
# ...or straight from a CD image, e.g. the one a DOSBox install mounts
wartool -r war2.cue ~/Library/Application\ Support/Stratagus/data.Wargus

# Convert a single map
pudconvert mymap.pud [outdir]

# Launch the game (extracting first if needed)
wargus [--extract | --extract-no-gui | path/to/REZDAT.WAR | war2.cue]
```

CD images can be `.cue`/`.bin` pairs (any track layout, raw or cooked
sectors) or plain `.iso` files; the data track is read as ISO 9660 without
mounting anything.

`-v` converts the videos and needs an `ffmpeg` with libtheora (they are
skipped without it). `-r` saves the CD audio soundtrack and encodes it to
Ogg Vorbis (built in, no ffmpeg needed): from
a `.cue`/`.bin` image it copies the audio tracks out; from a physical CD it
needs `cdparanoia` (Linux only). Ripped music in `<cd>/music/*.wav|ogg` is
picked up as well. Installed copies of the game lack `SNDDAT.WAR` and
`MUDDAT.CUD`, which stay on the CD, so extract from the CD or its image.

## Checking against the original

`examples/compare.rs` compares two extraction directories by decoded
content (PNG pixels and palettes, gunzipped text), ignoring encoder
differences:

```sh
wartool-original /cd ref && wartool /cd new
cargo run --release --example compare -- ref new
```

## Differences from the C++ tools

Fixed bugs:

- `wc2-config.lua` used `music_extension = ".wav"` whenever nothing was
  converted to OGG, even when no music had been ripped at all. It now falls
  back to `.mid` (the extracted XMI music) in that case.
- The credits arrow cursor's palette was scaled twice (`<< 2` applied
  twice to a shared buffer). It is now scaled once.
- Corrupt or truncated input produces an error instead of out-of-bounds
  reads or writes.
- Ripped music is encoded to Ogg Vorbis by `wartool` itself instead of
  ffmpeg (which, built without libvorbis, writes FLAC into `.ogg` files).
- MIDI events at the same tick keep their order (stable sort). The
  original's unstable sort could put a note-off right after the note-on it
  should precede, silencing notes.

New:

- Extraction directly from CD images (`.cue`/`.bin`, `.iso`), including the
  CD audio soundtrack.

Not ported (yet):

- The Battle.net / GOG edition, which needs MPQ archive support (StormLib).
- CD audio ripping on Windows (MCI) and FreeBSD.
- The Windows extraction log tee and registry lookup of Stratagus.
- When not run from a terminal, the launcher shows a notice and runs the
  extractor in the background instead of opening an `xterm` / Terminal
  window.

## License

GPL-2.0, like Wargus. See `COPYING`.
