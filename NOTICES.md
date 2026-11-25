# Third-Party Notices and Provenance

The drivers in this repository are the Ferric DOS project's own work,
licensed GPL-2.0-or-later (see `LICENSE`), except where a section
below says otherwise. Several drivers were written against upstream
references — register documentation, specifications, and in one case
a codebase this one derives from. This file records what came from
where, and reproduces the notices the upstream licences require.

Register-level provenance comments inside the source files refer to
the parent project's vendored reference tree (Linux pin `c44104f3`);
that tree is not part of this repository.

## Wi-Fi: Atheros AR2413 code derived from Linux ath5k / OpenBSD ar5k (ISC)

The register definitions in `crates/gate1-svc/src/ath5k.rs` and the
generated register tables in `crates/gate1-svc/src/ath5k_initvals.rs`
derive from the Linux **ath5k** driver and the OpenBSD **ar5k** code
it was based on, both under the ISC licence. The ISC licence requires
the copyright and permission notices to appear in all copies, so they
are reproduced verbatim here.

From Linux `ath5k/reg.h`:

```
Copyright (c) 2006-2008 Nick Kossifidis <mickflemm@gmail.com>
Copyright (c) 2004-2008 Reyk Floeter <reyk@openbsd.org>
Copyright (c) 2007-2008 Michael Taylor <mike.taylor@apprion.com>

Permission to use, copy, modify, and distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

From Linux `ath5k/initvals.c`:

```
Copyright (c) 2004-2007 Reyk Floeter <reyk@openbsd.org>
Copyright (c) 2006-2009 Nick Kossifidis <mickflemm@gmail.com>
Copyright (c) 2007-2008 Jiri Slaby <jirislaby@gmail.com>

Permission to use, copy, modify, and distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

From the Atheros common code, Linux `ath/reg.h`:

```
Copyright (c) 2008-2009 Atheros Communications Inc.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

From Linux `ath5k/base.c`, whose header is BSD-style with a GPL
version 2 alternative (the BSD terms are the ones relied on here):

```
Copyright (c) 2002-2005 Sam Leffler, Errno Consulting
Copyright (c) 2004-2005 Atheros Communications, Inc.
Copyright (c) 2006 Devicescape Software, Inc.
Copyright (c) 2007 Jiri Slaby <jirislaby@gmail.com>
Copyright (c) 2007 Luis R. Rodriguez <mcgrof@winlab.rutgers.edu>

All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer,
   without modification.
2. Redistributions in binary form must reproduce at minimum a disclaimer
   similar to the "NO WARRANTY" disclaimer below ("Disclaimer") and any
   redistribution must be conditioned upon including a substantially
   similar Disclaimer requirement for further binary redistribution.
3. Neither the names of the above-listed copyright holders nor the names
   of any contributors may be used to endorse or promote products derived
   from this software without specific prior written permission.

Alternatively, this software may be distributed under the terms of the
GNU General Public License ("GPL") version 2 as published by the Free
Software Foundation.

NO WARRANTY
THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
``AS IS'' AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF NONINFRINGEMENT, MERCHANTIBILITY
AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL
THE COPYRIGHT HOLDERS OR CONTRIBUTORS BE LIABLE FOR SPECIAL, EXEMPLARY,
OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF
THE POSSIBILITY OF SUCH DAMAGES.
```

## CardBus bridge: Linux yenta / ToPIC headers as reference reading

The register definitions in `crates/gate1-svc/src/cardbus.rs` were
verified against two Linux PCMCIA files, used as reference reading:
`yenta_socket.c` (SPDX-License-Identifier: **GPL-2.0-only**) and
`topic.h` (dual-licensed **MPL-1.1 / GPL**). No code from either file
is copied into the driver; they served as the register documentation
for the ToPIC95-family bridge.

## USB core: derivative of USBDDOS — GPL-2.0-only

The OHCI host-controller core in `crates/gate1-svc/src/ohci.rs` is a
port and derivative of **USBDDOS** by crazii, which is licensed
**GPL-2.0-only**. Its header records the verification sources
(USBDDOS `HCD/ohci.h` & `ohci.c`, `CLASS/hub.h` & `hub.c`).

This section is therefore **GPL-2.0-only**. That is a deliberate,
explicit exception: the workspace metadata for this repository says
GPL-2.0-or-later, but that does **not** extend to the USB core — a
derivative of a GPL-2.0-only work cannot be relicensed by a metadata
line, and this notice does not smooth over the difference. If you
redistribute, treat `ohci.rs` (and the USB sections built on it) as
GPL-2.0-only.

## VBE: Trident register documentation

The register definitions in `crates/ferric-vbe/src/trident_2d.rs`
were verified from two upstream sources: the Linux **tridentfb**
driver (`tridentfb.c`, **GPL-2.0**) and the X.Org
**xf86-video-trident** driver (**MIT/X11**). The software 2D engine
in `crates/ferric-vbe/src/sw_2d.rs` is original work with no
third-party derivation.

## Filesystems: specification-derived parsers

- The **exFAT** parser and writer (`crates/ferric-exfat`) were
  written from Microsoft's published **exFAT File System
  Specification** (2019). No Microsoft code is included.
- The **NTFS** reader (`crates/ferric-ntfs`) takes its on-disk format
  knowledge from the **ntfs-3g / linux-ntfs** reverse-engineering
  corpus, the public record of a format that was never officially
  specified. No ntfs-3g code is copied.

## Audio: ALi M5451 references

The register definitions and reset sequences in
`crates/gate1-svc/src/ali5451.rs` were verified against the Linux
**ali5451** driver (`ali5451.c`, **GPL-2.0**). **SBEMU**
(**GPL-2.0-only**) is a listed design reference for the Sound Blaster
emulation work. The OPL synthesizer in
`crates/gate1-svc/src/sb_opl.rs` is an original implementation.

## Ethernet: Intel specification

The Intel 82550 driver in `crates/gate1-svc/src/lib.rs` is
spec-derived: it was written from Intel's **8255x Open Source
Software Developer's Manual**, not ported from an existing driver.

## External prerequisites (not included)

- **Jemm** (JemmEx / Jemm386) and **JLOAD** — the memory manager and
  module loader every JLM in this repository requires at runtime.
- **HDPMI32i** — the DPMI host used by parts of the parent project's
  resident configuration.

Both are separate projects with their own licences. They are runtime
prerequisites to document, not components of this repository, and
neither is bundled here.
