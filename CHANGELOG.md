# Changelog

All notable changes to this project are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.2] - 2026-11-16

### Added

- Job queue: files dropped while a conversion is running wait in line, and the
  status circle shows how many are waiting.
- A failed job shows a short reason with Copy error and Open log buttons, and a
  batch reports for example "4 of 5 done, 1 failed".
- Images and audio convert several files at once. Settings > General >
  Parallel files sets how many (1-8, default: CPU count capped at 4).
- The status circle shows time left and ffmpeg's encode speed.
- A Windows notification when a job that took 10 seconds or more finishes while
  the circle is closed or not always on top.
- ffmpeg runs at below-normal priority, and the PC stays awake during a job.
- Settings > General > Replace original (off by default) moves the source file
  to the Recycle Bin after a successful conversion.
- Hovering a slice shows the expected size (for example "~7.2 MB") or the new
  dimensions for a resize.

### Changed

- The progress ring in a batch shows overall progress instead of resetting for
  each file.

## [0.1.1] - 2026-11-16

### Changed

- Lower idle memory: the OpenGL, DWM and sound libraries load only when the
  settings window or a sound needs them, sounds are freed when the wheel
  closes, and the working set is trimmed after startup, update checks and the
  wheel closing.

### Fixed

- Installing ffmpeg or LibreOffice from Settings no longer fails when winget is
  not on PATH, and a failed install now shows the reason.

## [0.1.0] - 2026-11-15

### Added

- Radial conversion wheel: hold <kbd>`</kbd> while dragging a file to open it,
  then drop on a slice to convert, or drop in the center and pick a slice.
- Modify wheel on <kbd>Caps Lock</kbd> for changes that keep the format: size
  targets (8, 25 and 50 MB), resolution, frame rate, quality, metadata,
  rotation, volume, loudness normalization, silence trimming, mono and mute.
- Image, video, audio and GIF conversion through ffmpeg; images to one
  multi-page PDF; PDF pages to PNG or JPG through the Windows PDF renderer;
  documents, and PDF to DOCX, through LibreOffice.
- Status circle with animated icons for preparing, converting (with a
  progress ring) and done, start and done sounds, and Show in folder and Copy
  to clipboard buttons. It closes itself after 1.5 seconds.
- Batches and folders, with the most common file type picked for the wheel.
- Presets: custom slices for either wheel.
- Optional copy to clipboard when a job finishes.
- Hardware encoding with NVENC, Quick Sync or AMF, and fast remux between MP4,
  MOV, MKV and M4V.
- Optional Explorer menu entry, Convert with Crystal Converter.
- Tray icon with update check, recent conversions, undo last conversion,
  settings, launch at login, always on top and quit.
- Settings window with General, Hotkeys, Presets and About tabs: key
  recorder with reset, install buttons for ffmpeg and LibreOffice, and the
  crystal-design motion (smooth scroll, animated numbers, gliding sliders,
  spring switches, gliding tabs and button icon feedback).
- Release zip with ffmpeg bundled, plus a standalone exe.

[Unreleased]: https://github.com/joezhuo2/crystal-converter/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/joezhuo2/crystal-converter/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/joezhuo2/crystal-converter/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/joezhuo2/crystal-converter/releases/tag/v0.1.0
