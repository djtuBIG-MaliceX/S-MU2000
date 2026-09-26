# Extracting the ROMs

S-MU2000 needs **ROMs taken from your own MU2000**. This folder holds the
procedure and the tools. The ROMs themselves are not distributed.

> **Nothing you produce here may be published or shared.**
>
> * the extracted ROM images you put in `roms/` (wave ROM, program ROM, LCD font)
> * the dumper firmware images (`firmware_*.bin`) and `.ydl` files that appear in `build/`
> * the unpacked Yamaha updater
>
> Do not redistribute them on GitHub, and do not attach them to Issues, Pull
> Requests, Discussions or Releases. Publishing the tools (source code) and
> sharing what they produce or extract are two different things. See the
> [notice in the README](../../README.md#hardware-derived-data-do-not-distribute-or-post).

Two things are needed.

| | Size | How to get it |
|---|---|---|
| Program ROM | 4 MB | **No extraction needed.** Rebuilt from the updater Yamaha publishes |
| Wave ROM | 32 MB | About 36 minutes over one USB cable. No disassembly, no MIDI interface |

## 1. Program ROM (no extraction)

The `.ydl` files inside Yamaha's updater
[`mu2r1_uw.zip`](https://jp.yamaha.com/support/updates/mu2r1_uw.html) are
Standard MIDI Files with the first four bytes replaced, and their contents
are the very SysEx messages that get written to Flash. The original image
can be reassembled from them.

```bash
python tools/dump/ydl_extract.py part1/images/v200U12k.ydl part2/images/v200u22k.ydl \
       -o roms/mu2000_flash.bin
```

The result matches the SHA1 registered in MAME. Details in
[rom-dump.md](rom-dump.md).

## 2. Wave ROM (over USB, about 36 minutes)

**A home-made firmware is written to the unit.** The procedure is in
[usb.md](usb.md).

```bash
# Build the dumper on top of your own roms/mu2000_flash.bin
python tools/dump/build_firmware.py --module usbdump --words 64 \
       --base roms/mu2000_flash.bin -o build/firmware_usbdump64.bin
python tools/dump/make_ydl.py --image build/firmware_usbdump64.bin \
       --ref roms/updater/x/mu2r1_uw/part2/images/v200u22k.ydl \
       -o build/usbdump64.ydl

# Flash it and dump
python tools/dump/stage_ydl.py build/usbdump64.ydl
build/upgrade_dumper/Upgrade.exe                        # about 12 minutes
python tools/dump/recv_dump.py --port "Yamaha MU2000-1" --prime-port "Yamaha MU2000-1" \
       --words-per-block 64 --out roms/dump             # about 36 minutes
python tools/dump/verify_roms.py roms/dump
```

**The dumper `.ydl` is not distributed.** It contains the genuine firmware
plus the home-made dumper, so distributing it would mean redistributing
Yamaha's firmware. Build it yourself as above. That the build is correct has
been confirmed by checking that a `.ydl` made with nothing but the scripts
in this repository matches a proven one byte for byte.

### Read this first

Only **the main firmware region (0x040000-0x3DFFFF)** is rewritten. The
downloader (0x000000-0x00C001) is not touched. So the download mode entered
by holding [Drum]+[PLAY]+[VALUE+] at power-on always stays alive, and the
genuine `mu2r1_uw.zip` can restore the unit at any time. This has been
confirmed on real hardware.

**It is still a firmware rewrite, and you do it at your own risk.** Before
you start, keep the genuine updater to hand and confirm that you can enter
download mode, so that you are not stuck if something goes wrong.

When things do not work, narrow it down with the probe firmwares in
[usb.md](usb.md) (`make_usbprobe.py` / `make_usbinit.py` / `make_usbhook.py`).

## 3. Fallback route (over MIDI, about 3.8 hours)

If USB does not work out, the same thing can be done over the DIN MIDI
ports. See [softdump.md](softdump.md) and [procedure.md](procedure.md).

Dumps taken over USB and over MIDI have been confirmed to **match to the
last byte**, so the two independent routes cross-check each other.

## 4. LCD font and sine table

The LCD characters come from the HD44780's built-in font ROM. If you do not
have it, `make_standins.py` makes a substitute (close in appearance, but not
the real thing).

The sine table used by the MEG is approximated by the same script. **It does
not match the table in the real chip**, so LFO waveforms differ slightly.

```bash
python tools/dump/make_standins.py
```

## Where things go

Arrange the extracted files the way S-MU2000 reads them.

| File | Contents |
|---|---|
| `roms/mu2000_flash.bin` | program ROM, 4 MB |
| `roms/dump/xv364a0.ic49` and three more | wave ROM, 4 × 8 MB |
| `roms/standin/sin-table.bin` | the MEG's sine table, 64 KB |
| `roms/hd44780u_b04.bin` | LCD font (the stand-in is used if absent) |

`roms/` is in `.gitignore`. **Never publish what you extracted** (see the
notice at the top).

## Reference

| | |
|---|---|
| [hardware.md](hardware.md) | Inside the unit: board, chips, address map |
| [updater-protocol.md](updater-protocol.md) | The `.ydl` format and the download-mode procedure |
| [usb.md](usb.md) | Investigation of the USB microcontroller (M37640) and the USB dumper |
| [softdump.md](softdump.md) | The MIDI dumper |
| [procedure.md](procedure.md) | Step-by-step for the MIDI route on the unit |
| [rom-dump.md](rom-dump.md) | Reading the chips after desoldering (for reference) |
| [hello.md](hello.md) | First steps, starting from lighting an LED |
