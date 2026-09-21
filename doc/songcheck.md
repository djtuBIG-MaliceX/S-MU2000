# songcheck: finding wrong-sounding voices in real songs

Most people running S-MU2000 have no MU2000 to compare against. What they do
have is songs. `tools/songcheck.py` plays a song through the emulator, solos
each channel, and looks for the faults that have actually been reported in
issues: a voice that makes no sound, a layer a semitone off, a note that dies
while held, a click repeating at a loop point, a sound that never stops,
clipping, DC offset. Each finding names the bank, program, note and time, and
points at a solo MIDI file that reproduces it, so it can go straight into an
issue.

It is a filter, not an oracle. Every row is a candidate for a listen.

```
python3 tools/songcheck.py song.mid [more.mid ...]    check songs (or a directory of them)
python3 tools/songcheck.py --sweep gm                 play all 128 GM voices once and check each
python3 tools/songcheck.py --sweep kits               the 31 XG drum kits
python3 tools/songcheck.py --sweep list.txt           your own list, "msb lsb prog" per line
python3 tools/songcheck.py --selftest                 test the analysers on synthetic signals (no ROMs)
python3 tools/songcheck.py --e2e                      test the whole pipeline through the renderer (ROMs)
```

Options: `--roms DIR` (default `roms`, or `SMU2000_ROMS`), `--render PATH`
(default `build/render`), `--out DIR` (default `build/songcheck`), `--no-solo`
(whole mix only, fast, finds less), `--json` (also write `report.json`),
`--reuse` (re-analyse the solo renders already in `--out` instead of
rendering again; each render leaves a `.boot` sidecar with its offset, so
tuning the analysers costs seconds, not hours).
Needs Python 3 and numpy. Standard MIDI Files, plain or RIFF-wrapped
(`.rmi`); convert RCP/XWS first. The report is rewritten after every song,
so an interrupted run keeps what it found.

## What it looks for

| kind | how | what it usually means |
|---|---|---|
| `silent` | a voice's notes at velocity 40 or more leave the solo track under 20 LSB RMS | bank/program not found, a voice that never keys on |
| `pitch` | on stretches where one note sounds alone, the fundamental is compared with the written note; flagged when the median over two or more notes is more than 35 cents off, octaves ignored. The harmonic product spectrum picks the peak; below 500 Hz autocorrelation refines it (short windows smear low notes), and the two must agree within 50 cents or the window is discarded | wrong tuning table, wrong sample rate, pitch envelope |
| `detuned-layer` | on those same stretches, a second harmonic series one semitone above or below carries more than half the energy of the main one | a layer transposed 100 cents instead of 1 cent (issue 3, JazzyBa2) |
| `loop-click` | jumps the waveform's own period does not explain, recurring at a fixed interval longer than 20 ms | a bad loop point or sample decode |
| `clicks` | six or more unexplained jumps while a note is held | decode glitches, envelope steps |
| `dies-early` | for organs, strings, choir, brass, reeds, pipes, leads and pads: a note held 1.5 s or more is silent before note-off | an envelope that ends too soon |
| `stuck` | 3 to 4 s after the channel's last note-off the solo track is still above 100 LSB and not decaying | a note that never releases, a runaway delay (issue 3, Performance 002) |
| `clipping` | samples at full scale, in the mix or a solo | a level bug |
| `dc` | more than 200 LSB of DC while playing | the effects DSP leaving an offset (todo 3) |

Pitch and detune are skipped where they cannot be trusted: drum parts, the
SFX bank (MSB 64), GM programs 113 to 128 (percussion and effects) and 9 to
16 (bars and bells, whose partials are not harmonic), stretches with pitch
bend, channels that used the tuning RPNs or portamento. Detune also needs
at least 70 cycles in the window, so a semitone is resolvable. `dies-early`
ignores the SFX bank and drum kits, which are one-shots, and any note during
which the song itself fades: a Master Volume SysEx ramp (universal or XG), or
a CC7 or CC11 change. `silent` ignores notes that start with the master
volume already near zero. A silent SFX-bank
slot is reported but marked, since many slots are empty on the hardware
too. Clicks are judged against the note's own period, so a raw saw wave is
not a click train.

## What comes out

`build/songcheck/report.md`, one table per song:

```
| kind | ch | voice | notes | at | what | reproduce |
| detuned-layer | 6 | bank 69/0 PC 35 (bank 69/0) | C2 D2 | 41.2 s | a second harmonic series one semitone away carries 83% of the main one | `build/songcheck/song.ch06.mid` |
```

The reproduce file is the song with every other channel's notes removed and
all programs, controllers and SysEx kept, so the voice is set up exactly as
in the song. Attach it and the time to an issue. The rendered solo WAVs sit
next to it for listening.

## Testing the tester

`--selftest` synthesises signals with known faults and checks every analyser
fires on its fault and stays quiet on a clean tone, a raw saw wave and a
clean rendered note. `--e2e` runs two one-note songs through the real
renderer: a normal piano must produce no findings, the same piano at volume 0
must be reported `silent`. Run both after changing an analyser.

Baseline on this machine (firmware v2.01, MAME-verified ROMs): the GM sweep,
the kit sweep and the bundled demo song all come out clean. The 44 songs in
the SHORT, XMAS, HOLIDAY and PARK folders of Yamaha's XG MIDI Library gave
three candidates, and every rule above that looks like special pleading
was added because a real song produced a false positive without it: low
strings flagged 37 cents sharp by a coarse FFT, a tuba and a xylophone
flagged by harmonic-product confusion, SFX one-shots flagged for dying, a
honky-tonk piano flagged as detuned.

## Sweeping the XG banks

The 1,353 XG voices live in a table in the program ROM (`src/xg/voices.h`
reads it at run time), so there is no list to ship. To sweep a bank, write a
list file:

```
0 1 0      # GrandPno variation, bank 0/1
0 1 4
```

and pass it to `--sweep`. Around one second of audio is rendered per voice.
