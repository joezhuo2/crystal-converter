<p align="center"><img src="assets/banner.png" alt="Crystal Converter" width="100%"></p>

<p align="center">
  <a href="https://github.com/joezhuo2/crystal-converter/releases/latest"><img src="https://img.shields.io/github/v/release/joezhuo2/crystal-converter?logo=github&color=a8e0ff" alt="release"></a>
  <a href="https://github.com/joezhuo2/crystal-converter/releases"><img src="https://img.shields.io/github/downloads/joezhuo2/crystal-converter/total?logo=github&color=blue" alt="downloads"></a>
  <a href="https://github.com/joezhuo2/crystal-converter/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/joezhuo2/crystal-converter/ci.yml?branch=main&label=CI&logo=githubactions&logoColor=white" alt="CI"></a>
  <img src="https://img.shields.io/badge/platform-Windows-lightgrey" alt="platform: Windows">
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white" alt="Rust"></a>
  <a href="https://ffmpeg.org"><img src="https://img.shields.io/badge/ffmpeg-bundled-007808?logo=ffmpeg&logoColor=white" alt="ffmpeg"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/joezhuo2/crystal-converter?color=green" alt="license"></a>
</p>

<p align="center">
  <a href="#download">Download</a> •
  <a href="#how-it-works">How it works</a> •
  <a href="#features">Features</a> •
  <a href="#formats">Formats</a> •
  <a href="#settings-and-keys">Keys</a> •
  <a href="#screenshots">Screenshots</a> •
  <a href="#files">Files</a> •
  <a href="#development">Development</a> •
  <a href="#license">License</a>
</p>

> Crystal Converter is a file converter for the Windows tray. Start dragging a
> file, hold a key, and a wheel opens in the middle of the screen. Drop the
> file on a slice to convert it, or drop it in the center and pick a format
> afterwards. A second key opens a wheel for changes that keep the format:
> shrink a video to 8 MB, resize an image, even out audio loudness and more.
> When it's done, the wheel shows you where the file went and closes itself.

<p align="center"><img src="assets/demo.gif" alt="The wheel opening with video formats, MP4 highlighted, then collapsing into a status circle that shows conversion progress and a Done check with Show in folder and Copy buttons" width="80%"></p>

## Download

Get the latest build from
[Releases](https://github.com/joezhuo2/crystal-converter/releases/latest):

- **`crystal-converter_x.y.z_x64.zip`** (recommended): the app with ffmpeg
  next to it. Unzip anywhere and run `crystal-converter.exe`. Nothing is
  installed.
- **`crystal-converter_x.y.z_x64.exe`**: the app alone. It uses an
  `ffmpeg.exe` next to it, one installed with winget, or one on your `PATH`.
  Settings > About has an **Install** button if none is found.

Windows 10 or 11, 64-bit.

### First launch

Crystal Converter is not code-signed yet, so SmartScreen says "Windows
protected your PC" the first time. Click **More info**, then **Run anyway**.
The builds come from the public [release workflow](.github/workflows/release.yml),
so you can check what went into them.

The first launch opens Settings so you can see the keys. After that the app
lives in the tray.

### Updates

The app checks GitHub 30 seconds after it starts (turn this off under
Settings > General). The tray menu then reads **Up to date (vX.Y.Z)** or
**Update found (vX.Y.Z)**; click the update item to open the release page.
**Check for updates** in the tray or Settings > About checks right away.
Nothing downloads or installs on its own.

## How it works

1. Start dragging a file (or several, or a folder) from Explorer, the
   desktop or any app that drags files.
2. While dragging, hold <kbd>`</kbd> for the **Convert** wheel or
   <kbd>Caps Lock</kbd> for the **Modify** wheel. As soon as you move the
   mouse, the slices fill in with the formats that fit the file you're
   holding.
3. Then either:
   - **drop on a slice** to start right away, or
   - **drop in the center**, let go of the key, and click a slice, or
   - **drop anywhere else** to cancel. Clicking outside the wheel also
     cancels.
4. The wheel collapses into one circle that shows each stage with its own
   animated icon: preparing, converting (with a progress ring when the length
   is known) and done. A sound plays when the job starts and when it
   finishes.
5. When it's done, the circle shows the file name with two icon buttons:
   **Show in folder** and **Copy to clipboard** (hover for their names). If
   you don't use them within 1.5 seconds, the circle closes. Hovering keeps it
   open.

The new file is saved next to the original. A name that's taken gets a
number, so nothing is overwritten.

## Features

- **Two wheels.** Convert changes the format. Modify keeps the format and
  changes size, quality or sound, and adds a suffix to the name, such as
  `clip-8-mb.mp4`.
- **Slices that fit the file.** The type is read from the file's first bytes,
  not only its extension. A batch of mixed files uses the most common type
  and leaves the others alone.
- **Batches and folders.** Drop several files or a folder; the circle counts
  through them. Several images to PDF become one multi-page PDF. Images and
  audio convert several files at once (Settings > General > Parallel files).
- **Queue.** Start another conversion while one is running (hold the key and
  drag, or use the Explorer menu); it waits its turn and the circle shows
  how many are waiting.
- **Time left and speed.** While converting, the circle shows the time left
  and ffmpeg's encode speed under the progress ring.
- **Clear errors.** A failed job shows a short reason with **Copy error** and
  **Open log** buttons. A batch reports `4 of 5 done, 1 failed`.
- **Estimated size.** Hovering a slice such as 8 MB shows the size to expect
  (`~7.2 MB`), or the new dimensions for a resize (`~960×540`).
- **Notifications.** A Windows notification appears when a job that took 10
  seconds or more finishes while the circle is closed or not always on top.
- **Stays out of the way.** ffmpeg runs at below-normal priority, and the PC
  stays awake until the job ends.
- **Replace original** (off by default) moves the source file to the Recycle
  Bin after a successful conversion. Nothing is deleted permanently, and a
  failed job or an empty result never touches the source.
- **Presets.** Add your own slices in Settings > Presets, for example
  "Discord 10 MB" (MP4 under 10 MB) or "Web" (WebP, 1920 px, quality 80).
  Each preset picks a wheel, a file type, an output format and any of the
  changes below.
- **Copy to clipboard when done** (off by default) puts the results on the
  clipboard as files, ready to paste into Explorer, chat apps or email.
- **Hardware encoding** uses NVIDIA NVENC, Intel Quick Sync or AMD AMF for
  MP4 when one works, and falls back to x264.
- **Fast remux.** MP4, MOV, MKV and M4V swap containers without re-encoding
  when no changes are asked for.
- **Explorer menu** (off by default): right-click a file and choose
  **Convert with Crystal Converter** (under Show more options on Windows 11)
  to open the wheel without dragging.
- **Tray icon.** Click it for Settings. Right-click for **Check for updates**,
  the update status, **Recent** conversions (click one to show it in
  Explorer), **Undo last conversion** (moves the new files to the Recycle
  Bin), **Open settings**, **Launch at login**, **Always on top** and
  **Quit**.
- **Low overhead.** When idle the app is a tray icon and a keyboard hook, with
  no window and nothing drawing: about 2–4 MB of private memory and no CPU.
  The wheel window is created when you press the key and destroyed when it
  closes. ffmpeg runs as a separate process only during a job. Settings is a
  separate process that exits when you close it.

## Formats

The Convert wheel leaves out the format the file already has.

| You drop | Convert to |
|---|---|
| Image (PNG, JPG, WebP, AVIF, BMP, TIFF, ICO, HEIC, SVG, TGA) | PNG, JPG, WebP, AVIF, GIF, ICO, BMP, PDF |
| Animated GIF | MP4, WebM, WebP, PNG, JPG |
| Video (MP4, MOV, MKV, WebM, AVI, WMV, FLV, MPG, TS, 3GP) | MP4, WebM, MOV, MKV, AVI, GIF, MP3, M4A |
| Audio (MP3, WAV, FLAC, OGG, Opus, M4A, AAC, WMA, AIFF) | MP3, WAV, FLAC, Opus, OGG, M4A |
| Document (DOC, DOCX, ODT, RTF, TXT, HTML) | PDF, DOCX, ODT, TXT, HTML |
| Spreadsheet or slides (XLS, XLSX, ODS, CSV, PPT, PPTX, ODP) | PDF |
| PDF | PNG, JPG (one image per page), DOCX |

PDF pages are rendered by Windows itself at twice their size. Everything
else in the Documents rows, and PDF to DOCX, needs
[LibreOffice](https://www.libreoffice.org). Settings > About
shows whether it's installed and has an **Install** button (through winget)
if not. Transparent images converted to JPG, BMP or PDF get a white
background.

### Modify wheel

| File | Built-in slices |
|---|---|
| Image | Quality 75, 50%, 1920 px, 1080 px, No metadata, Rotate |
| Video | 8 MB, 25 MB, 50 MB, 1080p, 720p, 30 fps, Mute, Rotate |
| Audio | +6 dB, -6 dB, Normalize (−14 LUFS), Trim silence, Mono, 128 kbps |

Size targets work out a bitrate from the length of the file. A file that's
already under the target is re-encoded at normal quality instead. Presets can
combine any of: output format, max size, longest edge, scale, quality, audio
bitrate, frame rate, volume, rotation, loudness normalization, silence
trimming, mono, mute and stripping metadata. A wheel shows up to 10 slices.

## Settings and keys

| Key | Does |
|---|---|
| Hold <kbd>`</kbd> while dragging | Open the Convert wheel |
| Hold <kbd>Caps Lock</kbd> while dragging | Open the Modify wheel |
| Release the key before dropping | Close the wheel |
| <kbd>Esc</kbd> | Close the wheel, or cancel a running job |
| Right-click the wheel | Close it |

The keys only take effect while a mouse button is held, so typing <kbd>`</kbd>
or toggling Caps Lock works as usual the rest of the time. To change them,
open Settings > Hotkeys, click the key and press a new one; a reset button
appears next to a key that isn't the default. Shift, Ctrl, Alt and Win can't
be used because they change what a drag does (copy, move or shortcut).

Settings has four tabs:

- **General:** how long Done stays up, sounds, copy to clipboard when done,
  save next to the original or to a folder, hardware encoding, parallel
  files, replace original, launch at login, always on top, the Explorer menu and update checks.
- **Hotkeys:** the two keys.
- **Presets:** add, reorder, delete or restore presets, and edit each one.
- **About:** version, whether ffmpeg and LibreOffice were found (with
  **Install** buttons), update check, the Releases page and the config
  folder.

Changes save as you make them, and the tray app picks them up right away.
The Settings window uses the same motion as
[crystal-design](https://crystal-design.vercel.app): smooth scrolling, animated
numbers, sliders that glide to where you click, spring switches, a gliding
tab indicator, and buttons whose icon turns into a check when they work.

## Screenshots

<p align="center">
  <img src="assets/wheel-convert.png" alt="Convert wheel for a MOV file with MP4 highlighted" width="49%">
  <img src="assets/wheel-modify.png" alt="Modify wheel for an MP4 file with the 25 MB slice highlighted" width="49%">
</p>
<p align="center">
  <img src="assets/wheel-converting.png" alt="Status circle at 42 percent with a progress ring" width="49%">
  <img src="assets/wheel-done.png" alt="Done circle with Show in folder and Copy to clipboard buttons" width="49%">
</p>
<p align="center">
  <img src="assets/settings-general.png" alt="Settings, General tab" width="49%">
  <img src="assets/settings-hotkeys.png" alt="Settings, Hotkeys tab" width="49%">
</p>
<p align="center">
  <img src="assets/settings-presets.png" alt="Settings, Presets tab" width="49%">
  <img src="assets/settings-about.png" alt="Settings, About tab" width="49%">
</p>

## Files

| Path | What |
|---|---|
| `%APPDATA%\crystal-converter\config.json` | Settings and presets. An unreadable file is kept as `config.json.bak` and defaults are used. |
| `%APPDATA%\crystal-converter\last-error.txt` | The full message from the last failed conversion (the **Open log** button). |
| `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` → `Crystal Converter` | Only while Launch at login is on. |
| `HKCU\Software\Classes\*\shell\CrystalConverter` | Only while the Explorer menu is on. |

Turning those two options off removes their registry entries. Nothing else
is written outside the app folder and the files you convert.

## Development

You need Rust (stable, MSVC toolchain) and ffmpeg on your `PATH` or next to
the built exe.

```bash
cargo run
```

```bash
cargo test
```

```bash
cargo test -- --include-ignored
```

The last one also converts real test clips with your ffmpeg. Open a settings
tab directly with `cargo run -- --settings presets`, and open the wheel for a
file with `cargo run -- --open path\to\file`.

Regenerate the banner, icon, demo GIF frames and wheel screenshots (the
wheel renderer runs without a window):

```bash
cargo run --release --example render_frames -- target/frames
```

| File | Does |
|---|---|
| `src/app.rs` | Single instance, hidden host window, message loop, tray actions |
| `src/hook.rs` | Low-level keyboard hook; only reacts while a mouse button is down |
| `src/wheel.rs` | Layered window, OLE drop target, state machine, job thread |
| `src/render.rs` | tiny-skia drawing for the wheel, status circle and icon |
| `src/convert.rs` | ffmpeg arguments, progress, image-to-PDF writer, LibreOffice |
| `src/formats.rs` | File type detection, targets and built-in slices |
| `src/settings.rs` | egui settings window and its animated widgets |
| `src/tray.rs`, `src/update.rs`, `src/sys.rs` | Tray menu, update check, registry, clipboard, sounds |

Pushing a `v*` tag runs the [release workflow](.github/workflows/release.yml),
which builds the exe, bundles ffmpeg and creates a draft release.

## Contributing

Issues and pull requests are welcome. Run `cargo fmt`, `cargo clippy
--all-targets -- -D warnings` and `cargo test` before opening a pull request;
CI runs the same checks.

## License

[MIT](LICENSE). The release zip includes an ffmpeg build from
[gyan.dev](https://www.gyan.dev/ffmpeg/builds/), which is licensed under the
GPLv3; its license and source link are in `FFMPEG-LICENSE.txt` and
`FFMPEG-NOTICE.txt` in the zip.
