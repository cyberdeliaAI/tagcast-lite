# Tagcast Lite

A lightweight, read-only iBroadcast desktop player. It's one native window with
your library, a queue and a now-playing bar. It needs no Python, FFmpeg,
Electron, browser or installed media player. On macOS it uses the system audio
output and the Keychain.

## Using it

On first launch the window shows an iBroadcast device code. Click
**Open iBroadcast**, enter the code and approve. The player only asks for
library and account read access. After that it opens straight into your
library from a verified, account-bound local copy and checks iBroadcast for changes in the background. Older sign-ins are bound to their account after their first successful status response.

- **Sidebar:** Songs, Albums, Album artists, Track artists, Queue and playlist folders. Browse by
  genre, tag or year, see favourites, or return to recently listened tracks.
  Search finds artists, albums and songs as you type.
- **Artist views:** Album artists shows album credits and Track artists shows
  the performers credited on songs. Cards open the albums for that role. Play
  on a track artist page queues that artist's credited songs. Both views have
  search, Name and Random sorting, and iBroadcast portraits where available.
  Click Name again to reverse the order, or Random to reshuffle.
- **Albums:** click a cover to open it. Hover a cover and click its
  play button to play it right away.
  Multi-disc sets have one cover and a
  section for each disc. Play follows disc and track order. Lite uses combined
  sets returned by iBroadcast and can group matching separate discs locally.
  The Albums sort buttons switch between
  artist, title, release year, date added, iBroadcast rating and random order.
  Click the active sort again to reverse it or reshuffle. Each change returns
  to the top of the grid. Date Added and Rating show the values below the covers.
- **Browse:** Genres, Tags and Years have searchable cards with album and
  track counts. Open a card to browse or play the matching albums.
- **Songs:** double-click to play from that song. Right-click for Play next,
  Add to queue, Go to album and Go to artist. Click a column header in Songs
  to sort.
- **Play and Shuffle** on an album, artist or playlist page replace the queue.
  The ⋯ button next to them adds everything to the queue instead.
- **Now-playing bar:** shuffle, previous, play/pause, next and repeat (off,
  all, one). Click or drag the position bar to seek. Click the title or artist
  to open the album or artist. The queue button opens a side panel on wide
  windows. System media controls receive track details and the playback clock.
- **Listening:** the queue is restored per account and waits for Play. The
  player records tracks locally after ten seconds of output. Mark local album
  favourites on the album page.
- **Quality:** 128 kbps is the default. With Premium, the menu at the bottom
  left offers 96, 192, 256 and 320 kbps and Original. A change restarts the
  current track at the same position.

Volume, quality, shuffle, repeat, album and artist sort, and the window size are remembered.

### Keyboard

| Key | Action |
| --- | --- |
| Space | Play or pause |
| ← / → | Seek 10 seconds |
| ⌘← / ⌘→ | Previous or next track |
| ⌘↑ / ⌘↓ | Volume |
| ⌘F | Search |
| Esc | Clear the search or go back |
| ⌘R | Reload the library |

On Linux, use Ctrl instead of ⌘.

### Command line

`--client-id ID` (or `IBROADCAST_CLIENT_ID`) selects a custom public OAuth
client. `--logout` removes the player's stored sign-in and library snapshot for
that client. **Sign out** in the ⋯ menu at the bottom left does the same from
the window. The full Tagcast app uses separate credentials and settings.

## Playback

The player streams audio into memory. It uses HTTP byte ranges when available.
A complete non-range stream is seekable from a bounded in-memory copy. If the
stream is still incomplete or exceeds the copy budget, the player stays where it
is and shows that the position is not yet available. Premium HLS supports the
iBroadcast MPEG-TS/AAC path and falls back to the direct stream of the same
bitrate when the segmented route fails. A temporary network interruption gets
one automatic recovery for the current track. Original may switch to 128 kbps
when the source or decoder rejects that format. The now-playing bar shows the
stream in use and any switch.

The sidebar menu offers an experimental **Gapless · Original FLAC** choice,
off by default. It applies to ordered album playback with compatible mono or
stereo FLAC tracks. The first track starts immediately. A bounded lookahead
prepares the following track and uses the same output and resampler through the
boundary. An unavailable or incompatible next track uses the ordinary transition.
The stream badge shows the decoded and output rates. Use **Reconnect audio
output** after switching speakers, headphones or a DAC. This opens the current
system output and leaves the track paused at its current position.

## Build and check

You need Rust 1.95 or newer (`rustup update`) and, on macOS, Apple's Command Line
Tools.

```sh
cargo build --locked --release
cargo test --locked
./target/release/tagcast-player
```

To get a double-clickable `Tagcast Lite.app` with the Tagcast icon:

```sh
scripts/make-macos-app.sh
open "target/release/Tagcast Lite.app"
```

The script signs the app ad hoc. Set `CODESIGN_IDENTITY` to your Developer ID
Application identity to sign it with that instead.

The Native builds workflow builds and packages Apple Silicon, Intel macOS, Windows
x64 and Linux x64. On Linux install the ALSA, D-Bus, OpenSSL, XKB and OpenGL
development libraries. Linux stores credentials in a private file under
`$XDG_DATA_HOME/tagcast-player` and uses MPRIS for desktop controls. Windows
stores credentials in Windows Credential Manager.

### Playback checks

```sh
cargo build --locked --release --example playback_checks
python3 -B tests/playback_fixture.py target/release/examples/playback_checks
```

These need FFmpeg and Python. They use temporary synthetic audio and a loopback
HTTP server. The report measures Play to the first decoded frames scheduled by
CoreAudio with the output muted and a fixed 40 ms fixture delay on the initial
HTTP response. RSS samples cover the playback process and its frameworks, with
no live library loaded. The accelerated run drains the same PCM queue into a test
sink and reports elapsed audio separately from wall time.

### Working on the interface

Debug builds can open a saved iBroadcast library response without signing in:

```sh
TAGCAST_PLAYER_FIXTURE=library.json \
TAGCAST_PLAYER_FIXTURE_AUDIO=http://127.0.0.1:8000/tone.wav \
TAGCAST_PLAYER_FIXTURE_ART=http://127.0.0.1:8000 \
cargo run
```

Every track then plays the given loopback file and covers load from
`<art server>/artwork/<id>-300`. `TAGCAST_PLAYER_FIXTURE=signin` shows the
sign-in screen with a sample code. Release builds leave all of this out.

## Credits

The interface uses [egui](https://github.com/emilk/egui) and the
[Inter](https://rsms.me/inter/) typeface (SIL Open Font License, see
`assets/fonts/Inter-LICENSE.txt`).
