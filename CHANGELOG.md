# Changelog

What changed in each version of RescueForge, in plain words.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [Semantic Versioning](https://semver.org/).

## [1.1.0] - 2026-11-24

### Added

- **Continue a stopped deep scan.** When you stop a deep scan, RescueForge remembers how far it got. A "Continue scan" button picks it up at the same place instead of starting over, and the files already listed and your ticks are kept. The position is also stored in saved sessions, so a long scan can be spread over several days: stop, save with Ctrl+S, open the session later and continue.
- **23 more kinds of file found by a deep scan** (66 kinds in all): JPEG 2000, DjVu, EMF drawings, icons and cursors, MPEG video (MPG, VOB), WavPack, Monkey's Audio, DSD, AMR voice recordings and AU audio, contacts (vCard) and calendars (iCalendar), Zstandard, LZ4 and Debian packages, fonts (TTF, OTF, WOFF, WOFF2), Flash, Windows shortcuts, registry hives and event logs, Linux programs and libraries, and network captures. As before, the real end of each file is worked out from its structure.
- **Previews for more files:**
  - Camera RAW photos (CR2, NEF, ARW, DNG and others) and Photoshop files show the picture stored inside them, also as thumbnails in the grid.
  - Songs (MP3, FLAC, M4A, WMA) show their cover picture.
  - Word, PowerPoint, Excel and OpenDocument files show their text, so you can tell which document is which before recovering.
  - ZIP archives (also JAR, APK, EPUB) list the files inside them.
  - Text files saved in the older Windows encodings (for example Arabic or Western European "ANSI") are shown as text instead of raw bytes.
- **Right-click menu on every file** in the list and in the thumbnail grid: recover this file, open the large preview, tick it, tick all files of the same type, tick everything in its folder, copy the name or the original path.
- **Export the list** of found files as a table (CSV) for Excel or any spreadsheet, with Ctrl+E or the "Export list" button: the ticked files, or everything shown. Like every other write, it is refused on the drive being scanned.
- **Search with several words and patterns.** `holiday 2025` finds names that contain both words; `*.jpg` or `img_20??.*` match whole names.
- **"Clear filters"** button next to the filters, and a **"Show everything"** button when no file matches.
- More keys: Page Up / Page Down, Home / End, and the arrow keys move tile by tile in the thumbnail grid.

### Changed

- **Kurdish, Arabic, Persian, Urdu and Hebrew are now drawn by Windows itself**, the same way File Explorer does: letters join and the text reads right to left. This covers file and folder names everywhere (list, grid, folder tree, details, recovery report) and the text preview.
- Stopping a deep scan now still lists the files of an earlier file system (from before a format) that were found up to that point. Before, they were dropped unless the scan ran to the end.
- The scan log says where a continued scan started and counts what was found since.
- The file-type list in Settings is longer; every new type can be switched off there.

### Fixed

- Kurdish and Arabic file names appeared as separate, unjoined letters in the wrong order.
- Moving through the list with the arrow keys could move the highlight out of sight: the list now scrolls with it.
- A very long file name that was shortened for recovery could end in a space or a dot, which Windows does not allow.
- The "No files match" message was a dead end; it now offers the way back.

### Not changed, on purpose

- The scanned drive, disk or image is still only ever opened for reading, and nothing is ever saved onto it.
- Session files from 1.0 open in 1.1 (they simply have no position to continue from).

## [1.0.0] - 2026-11-17

First release.

### Added

- Quick scan of NTFS, FAT12/16/32 and exFAT: deleted files with names, folders and dates, each rated good, partly overwritten or overwritten.
- Deep scan that reads every sector: 43 kinds of file found by their content, lost partitions, and files from before a quick format.
- Sources: drives, whole physical disks and disk image files; create a disk image first, skipping unreadable sectors.
- Results: folder tree with tick boxes, filters by type, size, date and recoverability, search, list or thumbnail grid, preview of pictures, text and raw bytes.
- Recovery to another drive with the original folders, each file checked by reading it back; refuses to write onto the drive being scanned.
- Pause, resume and stop; save a scan to a session file and reopen it later.
- Dark and light theme, demo mode, built-in self-test, installer and portable ZIP.

[1.1.0]: https://github.com/zheerkhalid/rescueforge/releases/tag/v1.1.0
[1.0.0]: https://github.com/zheerkhalid/rescueforge/releases/tag/v1.0.0
