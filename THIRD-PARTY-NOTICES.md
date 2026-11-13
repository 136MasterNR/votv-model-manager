# Third-party notices

This file covers the components that were added for importing `.7z` and `.rar` archives. Every other dependency is listed in
`Cargo.toml` / `Cargo.lock` with its own licence.

## UnRAR (rarlab), used through the `unrar` and `unrar_sys` crates

`.rar` import uses the UnRAR source code of rarlab (version 7.1, bundled by the `unrar_sys` crate and compiled into the
program). It is used only to UNPACK archives. The wrapper crates `unrar` and `unrar_sys` are licensed `MIT OR Apache-2.0`.
The licence of the UnRAR source code itself, unmodified:

```
 ******    *****   ******   UnRAR - free utility for RAR archives
 **   **  **   **  **   **  ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 ******   *******  ******    License for use and distribution of
 **   **  **   **  **   **   ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 **   **  **   **  **   **         FREE portable version
                                   ~~~~~~~~~~~~~~~~~~~~~

      The source code of UnRAR utility is freeware. This means:

   1. All copyrights to RAR and the utility UnRAR are exclusively
      owned by the author - Alexander Roshal.

   2. UnRAR source code may be used in any software to handle
      RAR archives without limitations free of charge, but cannot be
      used to develop RAR (WinRAR) compatible archiver and to
      re-create RAR compression algorithm, which is proprietary.
      Distribution of modified UnRAR source code in separate form
      or as a part of other software is permitted, provided that
      full text of this paragraph, starting from "UnRAR source code"
      words, is included in license, or in documentation if license
      is not available, and in source code comments of resulting package.

   3. The UnRAR utility may be freely distributed. It is allowed
      to distribute UnRAR inside of other software packages.

   4. THE RAR ARCHIVER AND THE UnRAR UTILITY ARE DISTRIBUTED "AS IS".
      NO WARRANTY OF ANY KIND IS EXPRESSED OR IMPLIED.  YOU USE AT 
      YOUR OWN RISK. THE AUTHOR WILL NOT BE LIABLE FOR DATA LOSS, 
      DAMAGES, LOSS OF PROFITS OR ANY OTHER KIND OF LOSS WHILE USING
      OR MISUSING THIS SOFTWARE.

   5. Installing and using the UnRAR utility signifies acceptance of
      these terms and conditions of the license.

   6. If you don't agree with terms of the license you must remove
      UnRAR files from your storage devices and cease to use the
      utility.

      Thank you for your interest in RAR and UnRAR.


                                            Alexander L. Roshal
```

## sevenz-rust2, used for `.7z` import

A 7z decompressor written in Rust, licensed `Apache-2.0` (https://github.com/hasenbanck/sevenz-rust). Only its reader is
used, with the LZMA / LZMA2 / BZip2 / PPMd / Deflate decoders; the program never calls its convenience extraction helpers
(older versions of those had a path-traversal bug, CVE-2026-61725, fixed in 0.21.1; the version used here is newer) and writes
every file itself after checking its path (`src/import.rs`).

