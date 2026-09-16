#!/usr/bin/env bash
# license:BSD-3-Clause
#
# scripts/bench_matrix.sh -- record blocktime numbers, with machine context,
# into doc/benchmarks.md (appends a dated section; never overwrites).
#
# NOTE: kept ASCII-only on purpose. macOS ships bash 3.2, whose parser breaks
# on multibyte characters inside parameter expansions.
#
# Configurations:
#   * arm64 native: both JITs / MEG JIT only / SH2 JIT only / no JIT
#   * x86_64 under Rosetta: both JITs / MEG JIT only / SH2 JIT only / no JIT
#
# Usage:
#   scripts/bench_matrix.sh [rom dir] [midi] [frames] [seconds] [repeats]
#   defaults: roms demo.mid 256 10 3
#
# Requires the ROMs and the MIDI file locally; neither is committed.

set -euo pipefail
export LC_ALL=C

ROM=${1:-roms}
MID=${2:-demo.mid}
FR=${3:-256}
SEC=${4:-10}
REP=${5:-3}
DOC=doc/benchmarks.md

# ---- machine context --------------------------------------------------------
model=$(sysctl -n hw.model)
cpu=$(sysctl -n machdep.cpu.brand_string)
osver=$(sw_vers -productVersion)
arch=$(uname -m)
ncpu=$(sysctl -n hw.ncpu)
pcore=$(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null || echo 0)
ecore=$(sysctl -n hw.perflevel1.physicalcpu 2>/dev/null || echo 0)
mem=$(( $(sysctl -n hw.memsize) / 1073741824 ))
ccver=$( ${CXX:-clang++} --version | head -1 )
rev=$(git describe --always --dirty)
rosetta=no
/usr/bin/pgrep -q oahd && rosetta=yes

# ---- build both flavors -----------------------------------------------------
echo ">> make blocktime (arm64)" >&2
make -j8 build/blocktime >&2

echo ">> make blocktime (x86_64, Rosetta)" >&2
make ARCH=x86_64 -j8 build-x86_64/blocktime >&2

# ---- run one config and pick the summary lines -------------------------------
# Blocktime prints, among others:
#   平均 0.674 ms（回ごとの幅 1.2%）  中央 0.66  95% 0.82  99% 0.90  最悪 1.34 ms
#   実時間に対する割合: 平均 46.5%  最悪 92%
#   ブロックの長さを超えた回数: 多い回で 4 / 6891
# blocktime lives in the flavor's build dir; run_in points at each in turn
run_in() { # $1 = binary path, $2 = label, rest = env
	local bin=$1; local label=$2; shift 2
	local out avg worst pct pctw over
	out=$(env "$@" "$bin" "$ROM" "$MID" "$FR" "$SEC" "$REP" 2>&1)
	avg=$(sed -n 's/.*平均 \([0-9.]*\) ms.*/\1/p' <<<"$out" | tail -1)
	worst=$(sed -n 's/.*最悪 \([0-9.]*\) ms$/\1/p' <<<"$out" | tail -1)
	pct=$(sed -n 's/.*割合: 平均 \([0-9.]*\)%.*/\1/p' <<<"$out" | tail -1)
	pctw=$(sed -n 's/.*割合:.*最悪 \([0-9.]*\)%.*/\1/p' <<<"$out" | tail -1)
	over=$(awk '/超えた回数/{sub(/.*多い回で /,""); sub(/ .*/,""); print}' <<<"$out")
	printf '| %s | %s | %s | %s | %s | %s |\n' \
		"$label" "$avg" "$worst" "$pct" "$pctw" "$over"
}

# ---- the matrix ---------------------------------------------------------------
rows=""
rows+="$(run_in ./build/blocktime          "arm64 native - both JITs")"$'\n'
rows+="$(run_in ./build/blocktime          "arm64 native - MEG JIT only" SMU2000_SH2_JIT=0)"$'\n'
rows+="$(run_in ./build/blocktime          "arm64 native - SH2 JIT only" SMU2000_MEG_JIT=0)"$'\n'
rows+="$(run_in ./build/blocktime          "arm64 native - interpreter" SMU2000_SH2_JIT=0 SMU2000_MEG_JIT=0)"$'\n'
rows+="$(run_in ./build-x86_64/blocktime   "x86_64 Rosetta - both JITs")"$'\n'
rows+="$(run_in ./build-x86_64/blocktime   "x86_64 Rosetta - MEG JIT only" SMU2000_SH2_JIT=0)"$'\n'
rows+="$(run_in ./build-x86_64/blocktime   "x86_64 Rosetta - SH2 JIT only" SMU2000_MEG_JIT=0)"$'\n'
rows+="$(run_in ./build-x86_64/blocktime   "x86_64 Rosetta - interpreter" SMU2000_SH2_JIT=0 SMU2000_MEG_JIT=0)"

# ---- append to the doc ---------------------------------------------------------
{
	echo
	echo "## $(date '+%Y-%m-%d %H:%M') -- $model ($cpu)"
	echo
	echo "- macOS $osver, uname $arch, $ncpu logical cores ($pcore performance + $ecore efficiency), ${mem} GB RAM"
	echo "- Rosetta 2: $rosetta; compiler: \`$ccver\`"
	echo "- source: \`$rev\`; block = $FR frames, $SEC s x $REP repeats, medians of runs"
	echo "- song: \`$MID\` (local only); ROM: \`$ROM\`"
	echo
	echo "| config | avg ms/block | worst ms | % of real time (avg) | % of real time (worst) | blocks overrun |"
	echo "|---|---|---|---|---|---|"
	printf '%s\n' "$rows"
} >> "$DOC"

echo ">> appended a section to $DOC" >&2
