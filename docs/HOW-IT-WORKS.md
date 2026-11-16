# How RescueForge works

This page explains, in plain language, what RescueForge does when it looks
for lost files, what the two kinds of scan are good for, and where the limits
are. Nothing here needs technical knowledge.

## Why deleted files can come back at all

A drive keeps two things: the contents of your files, and a table of contents
that says which file is where. When you delete a file (and empty the Recycle
Bin), Windows only crosses the file out in the table of contents and marks
its space as free. The contents stay where they were until something new
happens to be written to the same place.

That is the whole secret of file recovery, and also its main rule:

> **Stop using the drive.** Every new file, download, update or installation
> can land on top of what you lost. Recover to a *different* drive.

RescueForge takes this rule seriously. It opens the drive it scans for
reading only, and it refuses to save recovered files, disk images or scan
sessions onto the drive that is being recovered.

## Quick scan

A quick scan reads the drive's table of contents and looks for the entries
that were crossed out.

* On **NTFS** drives (most internal Windows drives) every file has a record
  in the "master file table". A deleted file's record is marked as unused,
  but it usually still holds the name, the folder, the dates and the exact
  places on the drive where the contents were stored, even when the file was
  stored in several pieces.
* On **FAT12, FAT16, FAT32** (older USB sticks and memory cards) a deleted
  file's entry is marked with a special first character. The name, size and
  starting position survive. The list of the pieces a file was stored in does
  not, so RescueForge assumes the file was stored in one piece. Short names
  lose their first character (`REPORT.TXT` comes back as `_EPORT.TXT`); long
  names come back whole.
* On **exFAT** (larger memory cards and USB drives) the entry survives with
  its name, size and position. The list of pieces usually survives too.

For every file it finds, RescueForge checks whether the space the file used
has been given to another file since:

| Rating | Meaning |
| --- | --- |
| **Good** | None of the file's space has been reused. It should come back intact. |
| **Partly overwritten** | Some of its space now belongs to another file, or its end could not be confirmed. It may open with damage, or not at all. |
| **Overwritten** | The beginning, or all of it, has been reused. The recovered file will almost certainly not open. |
| **Unknown** | The drive gave no way to tell. |

"Good" means the space was not *handed out* again. It cannot prove that the
bytes are untouched (see SSDs below), which is why previews are worth a look.

A quick scan takes seconds to a few minutes, because the table of contents
is small compared with the drive.

## Deep scan

A deep scan first does everything a quick scan does. Then it reads the drive
from the first sector to the last and looks at the contents themselves.

**Finding files by content.** Most file types start with a recognisable
signature: every JPEG begins with the same three bytes, every PDF with
`%PDF`. When RescueForge sees such a start, it does not simply cut at the
next signature. It reads the structure of the file the way a program opening
it would (the segments of a JPEG, the chunks of a PNG, the boxes of an MP4,
the directory at the end of a ZIP, the allocation table inside an old Word
document) to find out where the file really ends. Files found this way have
no name, folder or date, because those were only ever stored in the table of
contents. They are listed under "Found by content", grouped by type, with
generated names such as `recovered-000123456.jpg` (the number is the sector
where the file starts).

Types recognised by content: JPEG, PNG, GIF, BMP, TIFF, WebP, HEIC/AVIF, PSD,
camera RAW (CR2, CR3, NEF, ARW, DNG, ORF, RW2, PEF, SRW, RAF), MP4, MOV, M4V,
3GP, AVI, MKV, WebM, WMV/ASF, FLV, MPEG transport streams, MP3, WAV, FLAC,
OGG/Opus, M4A, WMA, MIDI, AIFF, PDF, DOCX/XLSX/PPTX (told apart by what is
inside), DOC/XLS/PPT/MSG, OpenDocument, EPUB, RTF, Outlook PST/OST, XML, SVG,
HTML, plain text (off unless you switch it on), ZIP, JAR, APK, RAR, 7z, GZIP,
XZ, TAR, CAB, EXE/DLL/SYS/MSI and SQLite databases. The list in Settings
lets you switch types off.

**Finding lost partitions.** Every volume starts with a "boot sector" that
describes it, and NTFS, FAT32 and exFAT keep a spare copy. If a partition was
deleted, or the partition table was damaged, the volume itself is usually
still there. RescueForge recognises these boot sectors (or their spare
copies) anywhere on the disk, and then reads that volume's own table of
contents. Its files come back with names and folders under "Lost partition".

**Finding files from before a format.** A quick format writes a new, empty
table of contents but leaves most of the old one, and all the old contents,
where they were. RescueForge collects the old NTFS file records it meets
while reading the drive and rebuilds the folder tree from them. These files
are listed under "Earlier file system".

By default a deep scan only looks in space that the drive's current table of
contents calls free. That is faster and avoids listing every file you still
have. If the file system itself is damaged, switch "search free space only"
off.

A deep scan takes as long as it takes to read the whole drive: minutes for a
memory card, hours for a large hard disk. You can pause, resume or stop it,
use the results while it runs, and save the result list to a session file
so that the scan never has to be repeated.

## Disk images

A disk image is a copy of a whole drive, sector by sector, in one file.
RescueForge can create one and can scan one exactly like a drive.

Make an image first when the drive is **failing**: clicking, disappearing,
reading very slowly, reporting errors. A failing drive may not survive being
read many times. With an image it is read once; every scan, preview and
recovery afterwards works on the copy. Sectors that cannot be read are
retried, then filled with zeros and listed in the scan log, and the copy goes
on. An image needs as much free space as the size of the drive.

## Honest limits

No recovery program can promise results. These are the cases where little or
nothing can be done, by RescueForge or by anything else:

* **SSDs with TRIM.** Solid-state drives are told by Windows which space is
  free, and they erase it by themselves, often within seconds of the delete.
  After that the drive returns zeros for that space. RescueForge shows a
  "SSD · TRIM" badge on such drives. Deleted files are frequently gone for
  good there; the rating may still say "Good" because the space was not
  given to another file, but the preview will be empty. Lost partitions and
  accidental quick formats are still worth a scan (a quick format also sends
  TRIM on many systems, so act fast and do not expect miracles). USB sticks
  and most memory cards do not use TRIM.
* **Overwritten data.** Once new data has been written over a file, the old
  contents are gone. There is no "deeper" level to read. A full (not quick)
  format, a secure-erase tool, or a drive that was filled up afterwards
  leaves nothing to find.
* **Files stored in pieces, found by content.** When a file is found only by
  its content, RescueForge cannot know where its second, third or tenth piece
  went. Such a file comes back complete only if it was stored in one piece.
  RescueForge marks files whose structure breaks off early as "Partly
  overwritten". Large files on full, long-used drives (videos, archives) are
  the most likely to be affected. Files found through the table of contents
  (quick scan, lost partition, earlier NTFS) do not have this problem on NTFS
  and exFAT.
* **Deleted files on FAT.** As described above, FAT forgets where the pieces
  of a deleted file were. Files that were stored in several pieces come back
  damaged.
* **Encrypted volumes.** A BitLocker (or other full-disk-encrypted) volume
  looks like random data from outside. Unlock it in Windows first and scan
  the unlocked drive letter; a scan of the locked volume or of the physical
  disk finds nothing useful. Files encrypted individually by Windows (EFS)
  are recovered still encrypted and need the original Windows account's key.
* **NTFS-compressed files** are found and rated, but RescueForge does not
  decompress them: the recovered copy holds the compressed data.
* **Formats without a recorded length.** FLAC audio and plain text do not say
  how long they are. Their end is estimated (where a blank sector or another
  file starts), so a few stray bytes may follow the real end.
* **Programs with attached data.** For EXE files RescueForge reads the size
  from the program's own headers. Installers that carry their payload behind
  the program part come back without it.
* **Very small files on NTFS** are stored inside their record in the table
  of contents. They come back perfectly as long as the record exists, but
  they cannot be found by content afterwards.
* **Dynamic VHD files and other virtual-disk formats** (VHDX, VMDK) store
  their blocks out of order and cannot be scanned directly. Fixed-size VHD
  files and raw images can.
* **Other file systems** (ext4, APFS, HFS+, ReFS) are not read through their
  table of contents. A deep scan still finds files by content on them.
* **Physically damaged drives.** RescueForge skips what cannot be read and
  recovers the rest. It cannot repair hardware. If the data is valuable and
  the drive is dying, a professional data recovery lab is the right address,
  and every further hour of power-on time lowers their chances.

## What RescueForge never does

* It never writes to the drive, disk or image it scans. They are opened for
  reading only; the part of the program that talks to drives has no write
  function at all.
* It never writes recovered files, images or sessions to the drive being
  recovered. A folder on a different volume of the same physical disk needs
  your explicit confirmation.
* It does not send anything anywhere. There is no network code in the
  program.
