# S-MU2000 User Guide

A walkthrough for **first-time users**: what you need, building, playing the standalone app, the screens,
and using it in a DAW. Internals and research notes are left out; each section points to the detailed
(Japanese) document. The Japanese version of this guide is [manual.md](manual.md).

> This is an unofficial project, not affiliated with Yamaha. Yamaha, MU2000 and XG are trademarks of Yamaha Corporation.

Contents

1. [What you need](#1-what-you-need)
2. [Preparing the ROMs](#2-preparing-the-roms)
3. [Building](#3-building)
4. [Standalone (gui.exe)](#4-standalone-guiexe)
5. [The front panel](#5-the-front-panel)
6. [PC editor windows](#6-pc-editor-windows)
7. [Using it in a DAW (VST3, CLAP, AU)](#7-using-it-in-a-daw-vst3-clap-au)
8. [Where settings are stored](#8-where-settings-are-stored)
9. [Troubleshooting](#9-troubleshooting)

---

## 1. What you need

| Item | Notes |
|---|---|
| An MU2000 | Needed to extract the ROMs. **ROMs are not distributed**; you take them from your own unit |
| A PC | Windows (x86-64) or macOS (Apple silicon). The whole MU2000 is emulated, so it uses real CPU (about 21% of real time on a 16-part song, Ryzen 7 9700X). **That share is machine-dependent** — it scales with single-core speed, so an older machine can be 2-3× that |
| A USB cable | For extracting the wave ROM (no MIDI interface needed) |
| A virtual MIDI cable | To drive the standalone app from a sequencer. On Windows, loopMIDI ([domino.md](domino.md)) |

**Never upload or share anything taken from the hardware (ROM images, firmware, `.ydl` files containing it).**
Hashes, logs, MIDI files and recordings are enough for bug reports. See the
[notice in the README](../README.md#hardware-derived-data-do-not-distribute-or-post).

## 2. Preparing the ROMs

Two ROMs are needed. Procedures and tools: [dump/README.md](dump/README.md).

| ROM | How to get it |
|---|---|
| Program ROM (4MB) | **No extraction needed.** Rebuild it from Yamaha's updater `mu2r1_uw.zip` with `tools/dump/ydl_extract.py` |
| Wave ROM (32MB) | About 36 minutes over one USB cable. A custom dumper is flashed to the unit and reads the ROM out (the official updater restores the unit at any time, but flashing is at your own risk) |

Put the results in one folder (the "rom folder"):

```
roms/
  mu2000_flash.bin        program ROM
  dump/xv364a0.ic49 etc.  wave ROM, 4 × 8MB
  standin/sin-table.bin   sine table used by the MEG
```

## 3. Building

Windows uses g++ and make from the MSYS2 MINGW64 environment; macOS uses Apple clang++ and make (C++20).
Linux builds too, but for now only the tools that need no window ([linux.md](linux.md)).

```bash
make
```

This produces, in `build/`:

| Output | What it is |
|---|---|
| `build/gui.exe` | The standalone app with a hardware-style panel (the one you will use most) |
| `build/S-MU2000.vst3/` | VST3 plugin |
| `build/S-MU2000.clap` | CLAP plugin (Windows) |
| `build/live.exe` | Plays incoming MIDI without a window |
| `build/render.exe` | Renders a MIDI file to WAV (**Standard MIDI Files only**, format 0 and 1; convert RCP/XWS to SMF first) |

The Windows executables do not depend on MSYS2 DLLs, so they run from plain PowerShell or Explorer.
macOS differences: [porting-macos.md](porting-macos.md).

## 4. Standalone (gui.exe)

### Starting

```bash
build/gui.exe C:\path\to\roms
```

After a few seconds the LCD shows the play screen (e.g. `◀000▶001 GrandP #01`), just like powering on the hardware.
**Later starts are faster** (the fully booted state is cached and reloaded).

### Choosing MIDI ports

**Left-click the `MIDI IN A` jack** drawn on the panel, or right-click anywhere in the window. You can change ports while running.

| Port | Parts |
|---|---|
| MIDI IN A | 1-16 |
| MIDI IN B | 17-32 |
| MIDI IN C | 33-48 (USB-only on the hardware) |
| MIDI IN D | 49-64 (same) |
| MIDI OUT | What the MU2000 itself sends (dump replies etc.) |
| MIDI THRU A / B | Passes received MIDI straight out (for A/B comparison with real hardware) |

To play from a sequencer such as Domino, create a port in loopMIDI and select it on both sides
([domino.md](domino.md)). Ports are remembered **by name**, so you only choose them once.

### Audio output and latency

Choose the output device at startup with `--audio "part of the name"` (list names with `build/gui.exe --list`).
It is remembered afterwards.

| Option | Effect |
|---|---|
| `--exclusive` | Exclusive mode. Lowest latency (measured 16ms); other apps cannot play audio meanwhile |
| `--latency 10` | Buffer length in milliseconds. Increase it if audio drops out |

Setting the audio interface to 44100Hz with a 256-sample buffer avoids resampling and extra delay.
Details: [README, 待ち時間 (latency)](../README.md#latency).

### Playing MIDI files

**Drop a MIDI file on a window**, or start with `--play song.mid`. Stop it from the menu of the card slot
("止める"). Songs with four ports are routed to A-D according to each track's port meta event (`FF 21`).

### Settings persist

MU2000 settings (utility settings, master volume, ...) survive restarts, like the battery-backed RAM of
the hardware. To reset to factory state, choose "工場出荷状態に戻す..." from the right-click menu, or start with `--factory`.

## 5. The front panel

It works like the real front panel; the LCD shows exactly what the firmware writes.

| Part | How to use |
|---|---|
| Buttons | Click. Also on the keyboard (table below) |
| Big dial | **Mouse wheel**, or drag up/down. Same as VALUE −/+ |
| VOLUME | Drag, or wheel over it. Final output level applied outside the synth |
| Card slot (bottom left) | Create / insert / eject SmartMedia; play and stop MIDI files |
| A/D INPUT jack | Choose the recording device used for sampling |
| PHONES jack | Choose digital or analog output behaviour |

| Key | Button |
|---|---|
| `A` `E` `U` `F` | PLAY, EDIT, UTIL, EFFECT |
| `Q` `M` | SEQ, SAMPLING |
| `S` | MUTE/SOLO |
| `[` `]` | PART −/+ |
| `-` `=` | VALUE −/+ |
| `,` `.` | SELECT ◀ ▶ |
| `Enter` `BackSpace` | ENTER, EXIT |
| `Z` `X` | AUDITION, SELECT |
| `F2` `F3` | Editor window, list window |

SmartMedia cards are PC files (`.img`); `tools/smcard.py` copies WAV files in and out.
**Sampling — recording through A/D INPUT and playing it back over MIDI — is walked through in [sampling.md](sampling.md)** (Japanese).
The panel artwork (positions, colours, SVG art) can be changed without rebuilding ([panel-editing.md](panel-editing.md)).
More about the screens: [gui.md](gui.md).

## 6. PC editor windows

Besides the panel there are mouse-and-keyboard windows for XG parameters. Closing one only hides it;
it reopens as it was. Details: [pc-editor.md](pc-editor.md).

| Window | How to open | Use it for |
|---|---|---|
| **List** | F3, right-click | Watching all parts while a song plays and balancing voices, levels and effects |
| **Part voice** | Double-click a VIB / FILTER / EG / EQ graphic in the list | Choosing a voice for one part and shaping vibrato, filter, EG and EQ by dragging points. Playable from the PC keyboard |
| **Insertion effect** | Double-click an INS badge, or INS 1-4 in the master row | Per-type parameters of insertion 1-4 as effect-pedal style knobs |
| **Master** | Double-click the MASTER name or the MASTER EQ graphic in the master row | Master volume, transpose, reverb / chorus / variation types and returns, master EQ |
| **Editor** | F2, right-click | A provisional table-and-knob layout (to be redesigned) |

### List

One row per part. VOL, PAN, VAR, CHO and REV can be changed by **dragging, the mouse wheel, or double-clicking to type**.
Right-click the part name for voices; right-click the INS cell to add or remove effects or choose the type.
INS badges (`1`-`4`, `V`) can be **dragged to another part** to move the effect. M mutes, S solos.
The keyboard can be played with the mouse. Polyphony (`発音 112/128 (M:64, S:48)`) and CPU load are shown top right.
Use `-` `+` at the top to change the display size.

### Part voice

* Pick a category and voice on the left. Each change plays a one-second audition note
* Drag points in the four graphics on the right (VIB: rate and depth; FILTER: cutoff and resonance; EG: attack, decay, release; EQ: frequency and gain). The sliders below do the same
* The top pane shows effect types, VOL-REV bars and a keyboard
* **Right-click a key to place a green marker**; auditions play that note
* The slot at the left end of the keyboard is a **modulation wheel**: hover and use the mouse wheel, or drag up/down
* **Play from the PC keyboard**: `A` `W` `S` `E` `D` `F` `T` `G` `Y` `H` `U` `J` `K` `O` `L` `P` `;` are chromatic from C. `Z` / `X` shift the octave down / up
* Clicking another row in the list switches this window to that part

### Insertion effect

The top strip switches INS 1-4 and chooses the type and the part it is applied to. Knobs: drag up/down (Shift for fine),
mouse wheel, or double-click to type. Types with EQ show a response graph.
Effect types carry a small icon per category (reverb, delay, distortion, EQ, ...).

### Help and language

Tick "説明を出す" (show help) at the top to get a description of the hovered item. Languages: Japanese and English.

## 7. Using it in a DAW (VST3, CLAP, AU)

The engine is the same as gui.exe, and so is **the screen** (right-click the panel to open the list and other windows).
Details: [vst3.md](vst3.md).

### Installing

```bash
make install-vst3
```

```bash
make install-clap
```

| Format | Location (Windows) |
|---|---|
| VST3 | `%LOCALAPPDATA%\Programs\Common\VST3` (current user) or `C:\Program Files\Common Files\VST3` (all users) |
| CLAP | `%LOCALAPPDATA%\Programs\Common\CLAP` or `C:\Program Files\Common Files\CLAP` |

On macOS the VST3 goes to `~/Library/Audio/Plug-Ins/VST3`; build the Audio Unit with `make au`.

### Telling the plugin where the ROMs are

The ROMs are 36MB, so instead of copying them, put a **`roms.txt` containing the folder path on one line**.

| Format | Where `roms.txt` goes |
|---|---|
| VST3 | `Contents/Resources/roms.txt` inside the bundle |
| CLAP | Next to `S-MU2000.clap`, or `%LOCALAPPDATA%\S-MU2000\roms.txt` |

```
C:\path\to\roms
```

The environment variable `S_MU2000_ROMS` also works.

### After inserting

* **It is silent for a few seconds** while the MU2000 boots, as with the hardware. MIDI received meanwhile is queued and played afterwards.
  The `Status` parameter goes `Booting` → `Ready` (`No ROM` means the ROMs were not found)
* There are **four MIDI input buses** (`MIDI In A (Part 1-16)` to `MIDI In D (Part 49-64)`); choose one per track
* `Output` is the output level
* An auxiliary audio input bus for A/D INPUT is available (hosts usually show it as a sidechain)

### Saving and automation

* Saving the project stores **voices, effects and everything edited on screen**
* Part volume, pan, sends, filter, EG, vibrato and EQ (64 parts × 19), master values (27) and
  insertion 1-4 parameters (4 × 16) are exposed as **named parameters** for automation
* Edits made on the plugin screen are recorded as automation too (VST3, CLAP)

Names and IDs: [automation.md](automation.md). Host-specific notes: [sonar.md](sonar.md), [reason.md](reason.md).

### plugin.ini

Write it at `%LOCALAPPDATA%\S-MU2000\plugin.ini` (macOS: `~/Library/Application Support/S-MU2000/plugin.ini`).
Takes effect when the plugin is loaded again.

| Line | Effect |
|---|---|
| `usb=0` | Start with the DIN ports (A and B, 32 parts) instead of the USB ports (A-D) |
| `threaded=0` | Run the second tone generator chip inside the host's audio thread instead of the plugin's own thread |

## 8. Where settings are stored

Windows: `%LOCALAPPDATA%\S-MU2000\`. macOS: `~/Library/Application Support/S-MU2000/`.

| File | Contents |
|---|---|
| `gui.ini` | gui.exe MIDI ports, audio output, VOLUME, inserted card, ... |
| `editor.ini` | PC window display size, help language, audition note, ... |
| `plugin.ini` | Plugin startup options (above) |
| `nvram\` | MU2000 settings (battery-backed RAM). Written by gui.exe and live.exe; plugins only read it |
| `boot\` | Cached booted state. Safe to delete; rebuilt on the next start |
| `log.txt` | Plugin log, including where ROMs were searched |
| `roms.txt` | ROM location for plugins (CLAP etc.) |

## 9. Troubleshooting

| Symptom | What to check |
|---|---|
| The plugin makes no sound | The `Status` parameter and `log.txt`. `No ROM` means check the path in `roms.txt` |
| No sound right after inserting | Boot wait (a few seconds), same as the hardware |
| Every voice in a song becomes piano | Voice selections arrived before boot finished. Wait for boot before playing |
| Parts 33-64 are silent | Make sure it started with USB ports (gui.exe without `--host-midi`; no `usb=0` in plugin.ini) |
| "MIDI が多すぎるので捨てた" (too much MIDI, dropped) | A MIDI loop between the DAW and loopMIDI. Check THRU routing |
| A port won't open / startup seems stuck | loopMIDI or a driver is hung. Restart loopMIDI or replug the device |
| Audio drops out | Increase `--latency`. Check the interface buffer and sample rate (44100Hz) |
| Start from a clean state | "工場出荷状態に戻す..." in the right-click menu, or `--factory` |

Report problems in [Issues](https://github.com/tarboh/S-MU2000/issues). Do not attach ROMs or firmware;
attach hashes, `log.txt`, MIDI files and recordings instead.
