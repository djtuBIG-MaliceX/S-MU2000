# Effect parameters (1–16) — SysEx addresses

`doc/effects.md` established, by measurement, the addresses for effect **type /
return / part / connection**, and `doc/params.md` lists the per-effect
**parameters 1–16** as not-yet-mapped. This fills that gap.

The addresses below were verified by **firmware round-trip** with Parameter
Requests (`F0 43 30 4C <ah> <am> <al> F7`, answered with
`F0 43 10 4C <ah> <am> <al> data… F7` — the same round trip `xgtest` uses): write
a value, request that address, and check it reads back. The per-effect layout
(which address stores each named parameter, its byte-size and range) is in
`src/xg/fx_params.h` and `doc/effects.md`.

## Addresses

| Block | Type | Param N (1..10) | Param N (11..16) |
|---|---|---|---|
| Reverb `02 01` | `00` (2B) | `02 + (N-1)` (1B) | `10 + (N-11)` (1B) |
| Chorus `02 01` | `20` (2B) | `22 + (N-1)` (1B) | — (not accepted) |
| **Variation** `02 01` | `40` (2B) | **`42 + 2·(N-1)`** (2B) | **`70 + (N-11)`** (1B) |
| **Insertion 1** `03 00` | `00` (2B) | **`02 + (N-1)`** (1B) *or* **`30 + 2·(N-1)`** (2B) — see below | **`20 + (N-11)`** (1B) |
| Insertion 2/3/4 | `03 01/02/03 00` | (same layout as Ins 1) | (same) |

`2B` = two data bytes (MSB, LSB); `1B` = one data byte.

**Reverb, chorus and variation use the insertion tables.** `tools/fxsweep/sysfx_check.cpp` takes each
type's insertion parameters (`src/xg/fx_params.h`), maps them to the addresses above
(`src/xg/sysfx.h`), writes the lower and upper limit and reads them back. All 196 reverb, 154 chorus and
1256 variation parameters came back as written, except parameter 10 (Dry/Wet) of reverb and chorus: it
always reads 0, because a system effect is mixed by its return level instead. Chorus P11–16 (`30–35`)
answer a Parameter Request but a write is dropped, and the work RAM has no room for them.

**In work RAM the three blocks are packed** (`src/xg/ram.h`): reverb `00–0D` then `10–15`; chorus `20–2E`;
variation `40–41`, then P1–10 as ten 16-bit values, then `56–60`, then `70–75`.

**Insertion P1–10 come in two forms, per effect type.** Most effects store P1–10
as **1-byte** values at `03 0n 02–0B`. Effects whose parameters need more than 7
bits — the delays, the `+DELAY` combos, V-Distortion — store P1–10 as **2-byte**
values at `03 0n 30–43` (MSB at the even address, LSB at the odd) instead.
`src/xg/fx_params.h` carries the per-type table (`size` = 1 or 2); an effect uses
one form or the other, not both.

**Insertion P11–16 are at `03 0n 20–25`, not `0D–12`.** `03 0n 0D–11` are the
insertion's MW / bend / CAT / AC1 / AC2 control depths (`0D` reads back `40`, the
depth default), so a "P11" write to `0D` would move the modulation-wheel depth
instead. P11–16 read back at `20–25` (`20` reads `28`, the same default the
variation's P11 shows).

## The one that bites: 2-byte params must be written atomically

The 2-byte parameters — every variation parameter, and the insertion parameters
that use the `30–43` form — are only accepted when **both data bytes arrive in a
single Parameter Change**:

```
F0 43 10 4C 02 01 <al> <MSB> <LSB> F7      # variation P1 = 0x0055:  ... 42 00 55 F7
F0 43 10 4C 03 00 <al> <MSB> <LSB> F7      # insertion 2-byte param, same shape
```

A single-byte write to the odd (LSB) address, or the MSB and LSB sent as two
separate messages, is **silently ignored** — the effect keeps running with its
default parameter. (This is why "set that FX param" can appear to do nothing.)
The 1-byte forms (reverb/chorus, insertion `02–0B` and `20–25`) take individually.

## Example — Variation = Tremolo, LFO Freq = 0x40, AM Depth = 0x7F

```
F0 43 10 4C 02 01 40 46 00 F7      # type = TREMOLO (2-byte, atomic)
F0 43 10 4C 02 01 42 00 40 F7      # P1  LFO Frequency
F0 43 10 4C 02 01 44 00 7F F7      # P2  AM Depth
```

Confirmed audible: tremolo AM Depth P2=0 → ~5 %, P2=127 → ~95 %. Undefined param
numbers for a given effect (e.g. Tremolo has no P4/P5) accept the write into
storage but the effect ignores them — expected.

## Verify

`build/fx_probe.exe <rom dir>` walks the variation + insertion parameter
addresses with Parameter Requests and prints, per address, the value that reads
back (confirming which addresses store). It needs ROMs and takes a few seconds
(no audio); add it to `make test` alongside `xgtest` if wanted.
