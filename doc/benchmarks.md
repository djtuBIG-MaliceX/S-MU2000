# Block-time measurements

A place to record how many milliseconds it takes to render one block of
64 frames (1.45 ms), together with the context of the machine that produced
it. If the percentage of real time stays below 100%, that configuration keeps
up in real time (for the margin below, look at the "worst" column). The floor
for latency is set by that worst value (doc/todo.md item 2). This measures the
render alone, without an audio device attached, so real latency gains whatever
the device buffers on top.

## How to measure

For a one-off run, use the script:

```
scripts/bench_matrix.sh                  # default: roms demo.mid 64 10 3
scripts/bench_matrix.sh roms my.mid 32 20 5   # different song and conditions
```

It appends a dated section with machine context to `doc/benchmarks.md`
(never overwrites). The MIDI file never enters git — it may carry an
incompatible license. Have the ROMs and the MIDI ready locally.

To check configurations one by one by hand:

```
# arm64 (the native build; both JITs compile in)
make build/blocktime
./build/blocktime roms demo.mid 64 10 3
SMU2000_SH2_JIT=0 ./build/blocktime ...   # MEG JIT only
SMU2000_MEG_JIT=0 ./build/blocktime ...   # SH2 JIT only
SMU2000_SH2_JIT=0 SMU2000_MEG_JIT=0 ./build/blocktime ...   # all interpreted

# x86_64 (Rosetta; both JITs compile in. ARCH= picks its own build dir)
make ARCH=x86_64 build-x86_64/blocktime
./build-x86_64/blocktime roms demo.mid 64 10 3

# arm64 JIT on/off switches
SMU2000_SH2_JIT=0 ./build/blocktime ...       # MEG JIT only
SMU2000_MEG_JIT=0 ./build/blocktime ...       # SH2 JIT only

# x86_64 JIT on/off switches
SMU2000_SH2_JIT=0 ./build-x86_64/blocktime ...   # MEG JIT only
SMU2000_MEG_JIT=0 ./build-x86_64/blocktime ...   # SH2 JIT only
SMU2000_SH2_JIT=0 SMU2000_MEG_JIT=0 ./build-x86_64/blocktime ...   # all interpreted
```

`blocktime <rom> <midi> <frames> [seconds] [repeats]`. The output includes the
spread across repeats plus 95%, 99% and worst-case figures. Level the Mac's
power and energy-saver settings before measuring.

---

## 2026-09-14 14:14 -- MacBookPro18,2 (Apple M1 Max)

- macOS 27.0, uname arm64, 10 logical cores (8 performance + 2 efficiency), 32 GB RAM
- Rosetta 2: yes; compiler: `Apple clang version 21.0.0 (clang-2100.3.34.2)`
- source: `e945ce2-dirty`; block = 64 frames, 10 s x 3 repeats, medians of runs
- song: `demo.mid` (local only); ROM: `roms`

| config | avg ms/block | worst ms | % of real time (avg) | % of real time (worst) | blocks overrun |
|---|---|---|---|---|---|
| arm64 native (interpreter) | 0.576 | 1.16 | 39.7 | 80 | 1 |
| x86_64 Rosetta - both JITs | 0.306 | 32.16 | 21.1 | 2216 | 23 |
| x86_64 Rosetta - MEG JIT only | 0.734 | 31.74 | 50.6 | 2187 | 45 |
| x86_64 Rosetta - SH2 JIT only | 0.527 | 1.90 | 36.3 | 131 | 27 |
| x86_64 Rosetta - interpreter | 0.947 | 2.16 | 65.2 | 149 | 127 |

## 2026-09-14 14:58 -- MacBookPro18,2 (Apple M1 Max)

- macOS 27.0, uname arm64, 10 logical cores (8 performance + 2 efficiency), 32 GB RAM
- Rosetta 2: yes; compiler: `Apple clang version 21.0.0 (clang-2100.3.34.2)`
- source: `494c20a`; block = 256 frames, 10 s x 3 repeats, medians of runs
- song: `demo.mid` (local only); ROM: `roms`

| config | avg ms/block | worst ms | % of real time (avg) | % of real time (worst) | blocks overrun |
|---|---|---|---|---|---|
| arm64 native (interpreter) | 2.256 | 3.09 | 38.9 | 53 | 0 |
| x86_64 Rosetta - both JITs | 1.167 | 56.62 | 20.1 | 975 | 10 |
| x86_64 Rosetta - MEG JIT only | 2.946 | 58.62 | 50.8 | 1010 | 18 |
| x86_64 Rosetta - SH2 JIT only | 2.114 | 4.73 | 36.4 | 82 | 0 |
| x86_64 Rosetta - interpreter | 3.892 | 7.53 | 67.0 | 130 | 37 |

## 2026-09-14 19:32 -- MacBookPro18,2 (Apple M1 Max)

- macOS 27.0, uname arm64, 10 logical cores (8 performance + 2 efficiency), 32 GB RAM
- Rosetta 2: yes; compiler: `Apple clang version 21.0.0 (clang-2100.3.34.2)`
- source: `0a0f98b`; block = 256 frames, 10 s x 3 repeats, medians of runs
- song: `demo.mid` (local only); ROM: `roms`

| config | avg ms/block | worst ms | % of real time (avg) | % of real time (worst) | blocks overrun |
|---|---|---|---|---|---|
| arm64 native - interpreter | 2.311 | 3.20 | 39.8 | 55 | 0 |
| arm64 native - SH2 JIT | 1.360 | 2.33 | 23.4 | 40 | 0 |
| x86_64 Rosetta - both JITs | 1.391 | 60.06 | 24.0 | 1035 | 10 |
| x86_64 Rosetta - MEG JIT only | 3.139 | 60.81 | 54.1 | 1048 | 14 |
| x86_64 Rosetta - SH2 JIT only | 2.242 | 4.72 | 38.6 | 81 | 0 |
| x86_64 Rosetta - interpreter | 4.049 | 7.96 | 69.7 | 137 | 49 |

## 2026-09-15 10:26 -- MacBookPro18,2 (Apple M1 Max)

- macOS 27.0, uname arm64, 10 logical cores (8 performance + 2 efficiency), 32 GB RAM
- Rosetta 2: yes; compiler: `Apple clang version 21.0.0 (clang-2100.3.34.2)`
- source: `a1d8c03`; block = 256 frames, 10 s x 3 repeats, medians of runs
- song: `demo.mid` (local only); ROM: `roms`
- note: first entry after the upstream merge. Both JITs now have arm64 backends,
  so the arm64 slice no longer interprets-only, and the MEG JIT rows are new
  there. Compared with the entry above, the interpreter got somewhat slower
  (2.311 -> 2.916 ms) from the code merged in, while both JITs together land
  well under it.

| config | avg ms/block | worst ms | % of real time (avg) | % of real time (worst) | blocks overrun |
|---|---|---|---|---|---|
| arm64 native - both JITs | 0.865 | 4.12 | 14.9 | 71 | 0 |
| arm64 native - MEG JIT only | 1.996 | 3.18 | 34.4 | 55 | 0 |
| arm64 native - SH2 JIT only | 1.719 | 5.69 | 29.6 | 98 | 1 |
| arm64 native - interpreter | 2.916 | 8.04 | 50.2 | 138 | 3 |
| x86_64 Rosetta - both JITs | 1.242 | 10.26 | 21.4 | 177 | 6 |
| x86_64 Rosetta - MEG JIT only | 3.317 | 11.25 | 57.1 | 194 | 16 |
| x86_64 Rosetta - SH2 JIT only | 2.402 | 6.02 | 41.4 | 104 | 2 |
| x86_64 Rosetta - interpreter | 4.638 | 8.99 | 79.9 | 155 | 100 |
