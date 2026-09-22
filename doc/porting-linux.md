# Linux GUI + plug-ins

> **Not maintained by the repository owner.** The Linux GUI and plug-ins were contributed by
> spessasus in PR #33. The maintainer does not use Linux and **cannot test, support, or take
> responsibility for them**. Use at your own risk; reports and fixes from Linux users are welcome.
> （作者は Linux を使っておらず、この部分の動作確認・保守・責任は負えない。自己責任で。）

The headless core (tools, `live`) is upstream's: [doc/linux.md](linux.md)
(issue #25) covers the platform setup, `build-linux/`, and ALSA audio/MIDI.
This file covers what sits on top: the SDL3 window (`build-linux/gui`) and
the ELF plug-ins. Details and progress: [doc/porting-linux-gui.md](porting-linux-gui.md).

```bash
sudo apt install build-essential libasound2-dev libcairo2-dev libfontconfig-dev libsdl3-dev
make          # everything below, into build-linux/
```

| Target | What |
|---|---|
| `build-linux/gui` | front panel + editor/effects faces + PC windows (F2/F3), in English |
| `build-linux/S-MU2000.vst3` | VST3 (`Contents/x86_64-linux/S-MU2000.so`), headless editor for now |
| `build-linux/S-MU2000.clap` | CLAP, headless editor for now |
| `build-linux/vst3probe`, `clapprobe` | headless hosts (`--view` waits for Phase 5) |

## On top of upstream

* `src/ui/midi_out_linux.cpp`, `src/ui/audio_in_linux.cpp`: the two ALSA
  backends issue #25 left out (THRU/MIDI-OUT, A/D INPUT). Same POSIX class
  shape as the macOS headers (`midi_out.h`, `audio_in.h` widened from
  `__APPLE__` to `!defined(_WIN32)`).
* `src/ui/texts.h`: display strings for the panel pages and engine log, in
  the author's Japanese by default. The Linux front end installs English at
  startup; Windows/macOS never call it and show exactly what they always did.
* `src/compat/gdi_linux.cpp`: the GDI subset over Cairo used by the shared
  panel sources.
* Plug-in portability fixes: `vst3/view.cpp` (CoreGraphics include),
  `vst3/plugin.cpp` (`clock_gettime`), `vst3/probe.cpp` (`dlopen`),
  `clap/plugin.cpp` (X11 window API).

`make test` passes unchanged (fingerprints are platform-independent), and
`vst3probe --torture` / `clapprobe` render byte-identical WAVs.
