<p align="center">
  <img src="packaging/io.github.coderastic.CatTube.svg" width="128" alt="CatTube">
</p>

# CatTube

A native Linux YouTube app made for TV screens and remotes.

CatTube is for the Linux box under your TV. It opens full screen, you use
it with the arrows, OK and Back on a remote or a keyboard (on CatosTV a
game controller works too), and there are no ads. Videos play in mpv, so 4K, HDR and 60 fps
work when your screen and graphics can take them.

## What it does

- **Your YouTube.** Home, Subscriptions, History, Playlists and Channels.
  You sign in the TV way: CatTube shows a code and you type it at
  google.com/device on your phone. More than one account works. Not signed
  in, Home shows what is live and the latest in gaming, news and sports.
- **Search** with suggestions as you type, and your recent searches when
  the box is empty. Results come as the matching channels first, then a
  grid of videos that keeps loading as you scroll.
- **A player made for the remote.** OK shows the controls, Left and Right
  skip 10 seconds, Up goes to the progress bar. Speed, quality, subtitles
  and audio tracks are a button away, chapters show on the bar, and you can
  change the quality without losing your place.
- **SponsorBlock.** Sponsor segments, intros, "please subscribe" reminders
  and anything else you pick are skipped and marked on the progress bar.
- **Dislikes are back**, from Return YouTube Dislike. Hold OK on a video to
  like it, subscribe, save it to Watch later or go to the channel.
- **Comments and live chat** next to the video.
- **Up next.** When a video ends the next one starts after a short
  countdown. Or it loops, or it just stops, your choice.
- **Your phone as a remote.** In the YouTube app on your phone go to
  Settings, Watch on TV, Link with TV code, and type the code from
  CatTube's Settings. The phone then picks what plays, and play, pause,
  seeking, volume and next stay in step on both.
- **History** lists what you watched, and CatTube remembers where you
  stopped in every video.
- **A mouse works too.** Point at something to pick it, click to open it.

## Install

**Flatpak.** Download `CatTube-1.0.4-x86_64.flatpak` from the
[releases](https://github.com/coderastic/CatTube/releases), then:

```sh
flatpak install --user CatTube-1.0.4-x86_64.flatpak
flatpak run io.github.coderastic.CatTube
```

Flatpak fetches the Freedesktop runtime from Flathub if you don't have it.

**AppImage.** Download `CatTube-1.0.4-x86_64.AppImage`, make it executable
and run it. It brings its own mpv; it needs Python 3 and curl, which almost
every distribution has.

**CatosTV** comes with CatTube.

**From source.** You need Rust, mpv, curl and Python 3.

```sh
cargo build --release
./target/release/cattube
```

The first time it starts, CatTube downloads its own copy of
[yt-dlp](https://github.com/yt-dlp/yt-dlp) and [Deno](https://deno.com)
(yt-dlp needs a JavaScript runtime for YouTube) into
`~/.local/share/cattube`, and keeps yt-dlp up to date from then on.

## Keys

| Remote / keyboard | In the lists | In the player |
|---|---|---|
| Arrows | move around | Left / Right skip 10 s, Up / Down between the bar and the buttons |
| OK, Enter | open, play | show the controls, press a button |
| Hold OK, F2 | the video's menu | |
| Back, Esc | back | hide the controls, then stop |

## Shared casting switch

On CatosTV, the Cast to this TV switch also controls CatTube's paired-phone
connection. Turning it off closes phone casting and blocks new pairing;
local browsing and playback remain available. Standalone installations keep
the usual phone controls. Other host shells can set
`CATTUBE_CAST_ENABLED_FILE` to a preference file containing
`cast_enabled = true` or `cast_enabled = false`; changes apply while running.

## Command line

| Option | What it does |
|---|---|
| `--section NAME` | open on home, subscriptions, history, playlists, channels or search |
| `--play VIDEO_ID` | play one video, then close |
| `--remote` | stay online for the phone app, without a window |
| `--tv-code` | print the code for Link with TV code |
| `--feed subscriptions` | print the newest videos from your subscriptions (for a TV home screen) |
| `--windowed` | not full screen |

## Your files

Settings, your accounts and the phone pairing are in `~/.config/cattube/`
(the accounts and the pairing only readable by you). What you watched,
where you stopped and your recent searches are in `~/.local/share/cattube/`,
and pictures are cached in `~/.cache/cattube/`. Signing out in Settings also cancels the
sign-in at Google.

## Thanks

- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [mpv](https://mpv.io) do
  the playing.
- [SponsorBlock](https://sponsor.ajay.app) and
  [Return YouTube Dislike](https://returnyoutubedislike.com) share their
  data with everyone.
- [yt-cast-receiver](https://github.com/patrickkfkan/yt-cast-receiver)
  showed how the phone remote talks.

## License

MIT, see `LICENSE`. CatTube is not made by or connected to YouTube or
Google.
