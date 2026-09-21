# VST3 plug-in

`S-MU2000.vst3` puts the MU2000 straight into a DAW. Sound is made the same
way as in `live.exe`: **the tone generator advances only as far as the host
asks**.

```
make                builds the bundle along with the executables
make vst3           the bundle only (build/S-MU2000.vst3/)
make install-vst3   copies it to the VST3 folder (may need administrator rights)
make probe          loads it without a DAW to check it works
```

## Location and ROMs

The bundle follows the VST3 convention.

```
S-MU2000.vst3/
  Contents/
    x86_64-win/
      S-MU2000.vst3      <- the DLL
    Resources/
      roms.txt           <- a note saying where the ROMs are (see below)
```

On Windows the host looks in two places.

| Location | Whose |
|---|---|
| `%LOCALAPPDATA%\Programs\Common\VST3` | this user only; no administrator rights needed |
| `C:\Program Files\Common Files\VST3` | everyone |

The ROMs cannot be bundled (each user extracts them from their own unit), so
the plug-in searches at start-up in this order and uses **the first place
where `mu2000_flash.bin` is found**.

1. the environment variable `S_MU2000_ROMS`
2. `<bundle>/Contents/Resources` and `roms` beneath it
3. next to the DLL, and `<next to the DLL>/roms`
4. the location written on the first line of `Contents/Resources/roms.txt` or `<next to the DLL>/roms.txt`
5. `%LOCALAPPDATA%\S-MU2000\roms`
6. `%USERPROFILE%\Documents\S-MU2000\roms`

The ROMs are 36 MB, so you will not want copies. **Writing the location on
one line of `roms.txt` is the easy way.**

```
C:\Users\you\GitHub\MU2000\roms
```

Whether they loaded, and where the plug-in looked, is recorded in
`%LOCALAPPDATA%\S-MU2000\log.txt`. The plug-in has no display of its own, so
when it makes no sound, look there first. On stop it also writes the CPU
usage and the worst time per block.

## Boot wait

From power-on to the moment it starts receiving, the MU2000 takes about two
seconds of audio (a little under two seconds of real time). Exactly as on
the hardware, if you send MIDI before that, the voice selections are dropped
and everything becomes piano.

The plug-in starts booting on a separate thread as soon as it is loaded and
returns silence until that finishes. MIDI that arrives in the meantime is
queued and sent in one go once booting is done. **If you insert it and play
immediately, there is no sound for a few seconds**, which is the same as
powering on the hardware.

## Clean-up on stop

When the host stops playback (`setProcessing(false)`, or `stop_processing` in
CLAP), All Sound Off and All Note Off are sent at the start of the next
block so that nothing is left ringing.

**They must not be sent to all 32 channels.** MIDI is serialised at the
hardware's 31,250 bps, so 2 ports × 16 channels × 6 bytes = 192 bytes take
**61 ms**, and the first note queued behind them is delayed by that much. In
a DAW, playing from the top of a song, the first note alone comes 30 ms or
more late and everything after it is in place
([issue #15](https://github.com/tarboh/S-MU2000/issues/15)).

So the plug-in remembers, per port, a 16-bit mask of the channels that have
sounded, and sends the clean-up only to those. `vst3probe --restart`
reproduces "stop and play again". Before the fix the first note was 33.9 ms
late; after it, 3.2 ms (the same as playing without stopping). That 3.2 ms
is the MIDI serialisation plus the firmware, and the hardware has it too.

### The remaining delay is the MIDI port's own speed (measured 2026-09-18)

At the top of a song a DAW **sends a bundle of initial controllers at the
same position as the first note** (volume, pan, bend, program and so on;
FL Studio's MIDI Out template does this). Nothing sounds until that bundle
has passed through the port. Measured with varying channel counts, the delay
is **exactly proportional to the byte count** (8 CCs + program + bend =
29 bytes per channel; `tools/latency/ccburst.py`).

| Channels | Bytes | USB port | DIN port |
|---|---|---|---|
| 1 | 42 | 4.2 ms | 12.9 ms |
| 3 | 100 | 7.3 ms | 31.4 ms |
| 8 | 245 | 14.8 ms | 77.8 ms |
| 16 | 477 | 26.9 ms | 152.1 ms |

* **USB**: `bytes / 19,500 + 2.1 ms` (the residual is 2.06 to 2.39 ms at all four points)
* **DIN**: `bytes / 3,125 - 0.6 ms` (31,250 bps; residual -0.58 ms at all four points)

19,500 bytes/s is **the value measured on the real unit's USB port**
(`doc/dump/usb.md`), so **a real unit connected over USB is delayed by the
same amount**. The plug-in uses the USB port by default (`usb=0` in
`plugin.ini` returns to DIN, the right-hand column above).

The only way to shorten this delay is to **place the initial controllers
earlier than the notes** in the DAW, and that is true of the hardware as
well.

## Threads

Audio is produced on the DAW's audio thread, but the SWP30 slave (the
MU2000's second tone-generator chip) alone runs on **a thread the plug-in
creates itself** (two cores per instance, about 20% faster). When the number
of instances exceeds a quarter of the logical cores, it automatically falls
back to one thread.

If you dislike threads the DAW does not manage, or want to leave core
allocation to the DAW, add this line to
`%LOCALAPPDATA%\S-MU2000\plugin.ini` (macOS:
`~/Library/Application Support/S-MU2000/plugin.ini`) and the slave runs
inside the DAW's audio thread too (the sound is identical). Shared by VST3,
CLAP and AU.

```
threaded=0
```

It takes effect on re-insertion (reopening the project). When it does, one
line appears in the log (log.txt).

## How MIDI arrives

VST3 does not hand over MIDI as it is. It splits into two.

| MIDI | How VST3 delivers it |
|---|---|
| note on/off, poly pressure, system exclusive | events (`IEventList`) |
| control change, pitch bend, channel pressure, program change | **parameter changes** (`IParameterChanges`) |

To receive the latter, `IMidiMapping` has to tell the host "channel × number
→ parameter id". That is why the plug-in declares 16 channels × 131 kinds =
2,096 parameters (flagged `kIsHidden` so they stay out of lists).

On receipt these are reassembled into MIDI bytes and passed to the tone
generator. **Repeated identical values must not be dropped.** RPN/NRPN rely
on sending the same value in sequence, `CC101=0, CC100=0, CC6=n`, and
thinning them corrupts the pitch-bend range.

When several arrive at the same instant, parameter changes go first and
events second, so that "set the volume, then sound the note" holds.

### Program change (Cubase does not use IMidiMapping)

REAPER and others turn a MIDI program change into a parameter through
`IMidiMapping` number 130 (`kCtrlProgramChange`), but **Cubase does not**. It
looks up the unit for the MIDI channel with `IUnitInfo::getUnitByBus` and
passes the program as an index into that unit's program list, to the
parameter flagged `kIsProgramChange` that belongs to the unit. With no unit
and no flag it silently drops it.

So the plug-in implements `IUnitInfo`.

- Under the root unit (0) sit 64 units (1-64), ports A-D × 16 channels, named `A Ch1` and so on
- One program list of 128 voices (`001`-`128`) is shared by all units
- Each channel's `Program` parameter (number 130) belongs to that channel's unit, with `kIsProgramChange` and `kIsList` set. The `IMidiMapping` assignment stays as it was

`build/vst3probe.exe <DLL> --torture` checks that every one of the 4 ports ×
16 channels connects "unit → 128-voice list → flagged parameter (the same as
`IMidiMapping` number 130)". Not yet tried in Cubase itself.

### Hosts that do not follow the table

Some hosts do not deliver as the table says. VSTHost 1.58 puts program
changes into an event (`DataEvent`, treated as system exclusive) instead of a
parameter, and pads the 2-byte `C0 xx` to a 3-byte `C0 xx 00`. Passing the
extra `00` to the tone generator makes it a data byte under running status,
program 0 follows, and the voice reverts to Grand Piano.

So if the first byte of a `DataEvent` is a channel status (`80`-`EF`), it is
treated as not system exclusive: exactly that message's length is taken and
the padding behind it is discarded. If the first byte is `F0`, or a fragment
(a data byte), it is passed through as bytes as before.
`build/vst3probe.exe ... --data-midi` imitates this host.

## Visible controls

The host's generic panel shows these two plus the XG value parameters
(below). The 4 ports × 2,096 MIDI parameters are hidden.

| Name | Contents |
|---|---|
| `Output` | output level 0-100%, applied outside the tone generator; slewed one sample at a time so sudden changes do not jump |
| `Status` | `Booting` / `Ready` / `No ROM`. Read-only. **Look here when there is no sound** |

`Status` exists because there is no other way to tell the few seconds of
boot silence from the silence of missing ROMs.

**XG value parameters** (part volume, filter, EG, EQ and so on, 64 × 19;
master, 27; insertion 1-4 parameters, 4 × 16) are also there. They can be
automated, and values touched in the plug-in's display are reported to the
host. Together with state saving (the XG value snapshot added in version 4)
they are described in [doc/automation.md](automation.md).

## Sample rate

The MU2000 only runs at 44,100 Hz. If the host uses anything else, a
windowed sinc (64 taps, Blackman window) converts. At exactly 44,100 no
conversion is applied.

The tone generator can produce "the samples ahead" on demand, so the
conversion adds no look-ahead latency. `getLatencySamples()` returns 0.

## Several instances

The ROMs are read-only, so the DLL holds one set and every instance shares
it. It is not 36 MB × instances. It is released when nobody is using it.

The tone generator itself runs once per instance. At around 40% CPU per
instance, this machine (measured) had headroom up to four at once.

Per DAW:

| DAW | Multitimbral | Notes |
|---|---|---|
| Cakewalk Sonar | **Yes.** Each MIDI track chooses its destination (bus A / B) and channel. 32 parts | [doc/sonar.md](sonar.md) |
| Reason | No. One track, one instrument, part 1 only. Also the case of a program change freezing the MIDI loop | [doc/reason.md](reason.md) |

## Four MIDI input buses

Like the hardware's MIDI IN A-D, there are **four input buses**.

| Bus | Reaches |
|---|---|
| `MIDI In A (Part 1-16)` | parts 1-16; inserting normally connects here |
| `MIDI In B (Part 17-32)` | parts 17-32 |
| `MIDI In C (Part 33-48)` | parts 33-48; USB-only on the hardware |
| `MIDI In D (Part 49-64)` | parts 49-64; likewise USB-only |

C and D only arrive **when HOST SELECT is USB**, which is the default (`usb=0`
in `plugin.ini` returns to DIN with A and B only; [doc/gui.md](gui.md)). Some
DAWs will not let you choose a bus beyond the second.

Since CC, pitch bend and program change arrive as parameters in VST3, there
are 16 channels × 131 of them per port. Numbering: A is 0-2095 (as before),
B from 8192, C from 16384, D from 24576. 8,386 in all. The output level (4096)
and status (4097) numbers are unchanged.

## A/D INPUT (audio input bus)

As the way in for audio to be sampled, there is **one auxiliary stereo audio
input bus, `A/D Input`**. Left is AD1, right is AD2. DAWs usually show it as
a sidechain input. Unconnected, it is silent. If the host rate is not
44,100 Hz it is converted internally first. It feeds sampling and the A/D
parts. The A/D part volume defaults to 0 in XG, so raise it to hear anything
(XG parameter 10 00 0B; AD2 is 10 01 0B). Input effects apply as on any part.

## SmartMedia

Clicking the **card slot** on the panel (bottom left) brings up the same menu
as gui.exe (create and insert / insert / eject). The card's contents are a
PC file (`.img`, the same form as gui.exe). What the firmware writes is
flushed every two seconds while the display is open, when the project is
saved, and when the plug-in is removed. The project remembers **only the
file's location** (contents are 16-128 MB, so they are not stored), and
reopening reinserts that file. Moving files in and out from the PC side is
`tools/smcard.py` (doc/gui.md).

Do not insert the same file in two plug-ins, or in a plug-in and gui.exe, at
the same time: the flushes collide and one of them is lost.

## Checking

`make probe` is a small host that loads the plug-in without a DAW. Given a
MIDI file, it writes a WAV through the VST3 path.

```
build/vst3probe.exe build/S-MU2000.vst3/Contents/x86_64-win/S-MU2000.vst3
build/vst3probe.exe <that DLL> song.mid out.wav --rate 48000 --block 128
build/vst3probe.exe <that DLL> --torture
build/vst3probe.exe <that DLL> --automation
```

`--automation` exercises the XG value parameters and state saving
([doc/automation.md](automation.md)).

The MIDI file's port (`FF 21`) is passed through as the bus number. With
`--one-bus` everything goes to bus A (to compare whether B is working). On
the `port_b` song, with two buses the envelope differs from `render.exe` by
1.0%; with `--one-bus`, 24.6%.

`--torture` does every rough thing a host might do: discard without
initialising, cycle initialize/terminate, every rate from 22,050 to
192,000, zero-length process, no buses, 64-bit requested, toggling setActive
rapidly, querying all 4,194 parameters, MIDI mapping for two ports, four
instances at once. Every DAW calls differently, so not crashing is checked
here first.

Compared against `render.exe` (export without going through VST3):

| Host rate | 50 ms windows matching (within 10% amplitude) | Windows where the reference sounds and this does not |
|---|---|---|
| 44100 | 98.3% | 0 |
| 48000 | 98.1% | 0 |

They do not match sample for sample: when MIDI arrives a few cycles
differently, the SWP30 channel it is assigned to changes. The sound is the
same.

## Display

A display styled on the hardware front panel is included (`IPlugView`
implemented in-house, one child window inside the host's parent). The
contents are the same as gui.exe. See [doc/gui.md](gui.md).

**Right-click the panel to open the same "list" and "editor" as gui.exe**
([doc/pc-editor.md](pc-editor.md)). See and touch the 64 parts' voices,
volumes and effect sends there. The same menu has the SmartMedia swap.

These are the same `ui::pc_window` as gui.exe, and **the plug-in owns those
windows** (only the panel fits inside the host's parent). Each window has
its own ImGui context, so the list and editor can be open together. Closing
hides rather than destroys, so reopening shows the same state. Redraw is the
panel's 30 frames per second, from the display thread. CLAP has the same.

## Not there yet

- individual outputs (the hardware's six); stereo only for now
- 64-bit float processing (`kSample32` only)

## macOS

The macOS build has the same contents. Only the bundle layout and the port
names differ.

```
make vst3                    builds build/S-MU2000.vst3
make probe                   loads it pretending to be a host
make install-vst3            copies to ~/Library/Audio/Plug-Ins/VST3

build/vst3probe build/S-MU2000.vst3                  inspect it
build/vst3probe build/S-MU2000.vst3 song.mid out.wav play a file
build/vst3probe build/S-MU2000.vst3 --torture        rough handling
build/vst3probe build/S-MU2000.vst3 --view 20        show the display
```

The bundle is `Contents/MacOS/S-MU2000` (the executable) and
`Contents/Info.plist`. Hosts open it with `CFBundle`, not `dlopen`, and call
`bundleEntry`. The display is `src/vst3/view_mac.mm` (an `NSView`) added
inside the host's parent view. Drawing goes through the same
`compat/gdi_mac.cpp` as the GUI, so **panel.cpp is the same source as the
Windows build**.

ROMs are searched for in `S_MU2000_ROMS`, then `roms.txt` next to the
bundle, then `~/Library/Application Support/S-MU2000/roms`.

An Audio Unit (AUv2, `aumu`) using the same engine can also be built
(`make au`). That is `src/au/plugin.cpp`.

The port as a whole is summarised in [porting-macos.md](porting-macos.md).
