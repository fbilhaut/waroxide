# waroxide

> [!WARNING]
> This project was massively vibe-coded. **Expect no support.**
> I just wanted to play Warcraft II with my kids on macOS. 
> Use it as is, at your own risk.

Play Warcraft II on a Mac: a self-contained `Waroxide.app` that turns your
own Warcraft II CD (or CD image) into a game you can play, alone or with
others on the local network.

Under the hood it is [Wargus](https://github.com/Wargus/wargus), the
Warcraft II mod for the [Stratagus](https://github.com/Wargus/stratagus)
engine, with its native tools rewritten in Rust:

| Binary       | Replaces                    | Does                                                        |
|--------------|-----------------------------|-------------------------------------------------------------|
| `wartool`    | `wartool.cpp`               | Extracts the original CD data into PNG/WAV/MIDI/Lua files   |
| `pudconvert` | `pudconvert.cpp`, `pud.cpp` | Converts a `.pud` map to Stratagus `.smp.gz` / `.sms.gz`    |
| `waroxide`   | `wargus.cpp` + launcher     | Extracts on first run, then starts Stratagus on the data    |

The game itself still runs on the Stratagus engine with the Wargus Lua
scripts. The extracted data is meant to match wartool 3.3.3's layout and
formats exactly, so data from either tool works with either launcher (see
"Checking against the original" below).

## Playing on macOS

### What you need

- A Mac with Apple Silicon, running the macOS version the app was built on
  or later (the build prints it; builds made on macOS 26 need macOS 26).
- Your own **Warcraft II DOS CD** (*Tides of Darkness*, optionally the
  *Beyond the Dark Portal* expansion), either as a mounted CD or as a CD
  image: `.cue`/`.bin` (like the one a DOSBox setup uses) or `.iso`. An
  image gives you the CD soundtrack too.
- Not supported: the Battle.net edition, which is what GOG sells. An
  installed DOS copy isn't enough either: the music and videos archives
  stay on the CD.

There is no prebuilt download for now: build `Waroxide.dmg` yourself (see
[Building the macOS app](#building-the-macos-app)).

### Install

Open `Waroxide.dmg` and drag **Waroxide** to **Applications**.

The app is not notarized by Apple. If you copied it from another Mac or
downloaded it, macOS refuses to open it the first time: try to open it,
then go to **System Settings → Privacy & Security** and click
**Open Anyway** (or run `xattr -dr com.apple.quarantine /Applications/Waroxide.app`).

### First launch

1. Open Waroxide. It asks for the game data: pick `REZDAT.WAR` on the
   mounted CD, or the CD image (`.cue`, `.bin` or `.iso`).
2. Wait while it extracts. It takes about half a minute, with no progress
   window. The game is converted, and with an image the soundtrack is
   encoded too.
3. It asks whether to extract an additional expansion CD. Answer **No**,
   or **Yes** and pick the *Beyond the Dark Portal* CD if you have it.
4. The game starts. From then on, opening Waroxide goes straight to the game.

The intro and cutscene videos are skipped unless `ffmpeg` with libtheora
is installed (for example through Homebrew); the game plays fine without
them.

### Playing together on a LAN

Every player needs Waroxide on their own Mac, ideally the **same build of
the app** (Stratagus refuses players whose engine or scripts differ), and
their own extracted data (CDs from different countries work together).

- One player chooses **Multi Player Game → Create Game** and picks a map.
- The others choose **Join Game**: games on the network are listed with
  "(auto-discovered)". Otherwise, use **Add server** with the host's IP
  address.
- The first time, macOS asks to allow Waroxide on the local network: allow
  it. With the macOS firewall on, the host must also accept incoming
  connections.

The game uses UDP port 6660.

### Where things are stored

- `~/Library/Application Support/Stratagus/data.Wargus`: the extracted game
  (about 170 MB, including a copy of the CD archives so it can be extracted
  again without the CD).
- `~/Library/Application Support/Stratagus/wc2`: settings and saved games.

The app itself is never modified. To start over (another CD, or after a
problem), quit the game and delete `data.Wargus`: the next launch asks for the
game data again. Deleting `wc2` also resets the settings and removes the
saved games.

## Building the macOS app

```sh
brew install cmake sdl2_mixer sdl2_image dylibbundler
macos/build-app.sh
```

This needs Rust and the Xcode command line tools, and produces
`target/macos/Waroxide.app` and `target/macos/Waroxide.dmg`. The app contains
the launcher and extractor, the Stratagus engine with its libraries, and the
Wargus scripts; Stratagus and Wargus are fetched at pinned commits. It is
ad-hoc signed only.

`macos/test-multiplayer.sh <data-dir>` checks LAN discovery and a 2-player
game between two instances of the bundled engine on one Mac.

## Building the command-line tools

```sh
cargo build --release            # native dialogs in the launcher (rfd)
cargo build --release --no-default-features   # console-only launcher
```

The launcher's install paths can be set at build time, as in the CMake
build (the macOS app needs none of them):

- `WARGUS_DATA_PATH`: an already-extracted, shared data directory to use if
  present (default `/usr/local/share/games/stratagus/wargus`). Otherwise,
  and for every new extraction, the per-user directory is used
  (`~/Library/Application Support/Stratagus/data.Wargus` on macOS).
- `WARGUS_SCRIPTS_PATH`: directory with the Wargus `scripts/`, `maps/`,
  `campaigns/`, `shaders/`, `contrib/` (defaults to the data path).
- `STRATAGUS_BIN`: the engine (default `stratagus`, looked up on `PATH`,
  then next to the launcher).

At runtime the scripts are looked up in `WARGUS_SCRIPTS_PATH` (if set,
e.g. a Wargus checkout), the app bundle's `Contents/Resources`, the
build-time path, then the folder containing `wartool`. On Windows, the
paths default to the launcher's own folder instead.

## Command-line usage

```sh
# Extract from the DOS CD (its root or its DATA directory)...
wartool [-e|-n] [-v] [-r] /Volumes/WAR2 ~/Library/Application\ Support/Stratagus/data.Wargus
# ...or straight from a CD image, e.g. the one a DOSBox install mounts
wartool -r war2.cue ~/Library/Application\ Support/Stratagus/data.Wargus

# Convert a single map
pudconvert mymap.pud [outdir]

# Launch the game (extracting first if needed)
waroxide [--extract | --extract-no-gui | path/to/REZDAT.WAR | war2.cue]
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
