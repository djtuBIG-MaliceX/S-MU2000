#!/usr/bin/env python3
# license:BSD-3-Clause
"""回帰試験を一息で回す。`make test` から呼ばれる。

  python tools/run_tests.py [--roms <ディレクトリ>] [--only 名前] [--update]

見るもの:

  1. verify.exe      SWP30 のレジスタ素通しと乱数の数列（ROM 不要）
  2. statetest.exe   状態の保存と復元。写し忘れがあれば落ちる
  3. 鳴らし比べ       tests/*.json の指紋と突き合わせる
  4. スレーブ別糸      threaded と --single で出る音が同じこと
  5. xgtest.exe      パラメータの層の定義表を firmware に読み返させる（doc/params.md）
  6. samptest.exe    パネルで録音して試聴し、録った音が返ってくるか
  7. JIT 入切       同じ曲を JIT あり・なしで鳴らし、wav がバイト単位で同じか
                    （JIT は解釈実行と同じことをするはずなので、ずれたら訳し方の間違い）

**ROM が無い機械では 1 番だけ走る**（ROM は同梱できないので、それが正しい）。
ROM の置き場は --roms、環境変数 SMU2000_ROMS、roms/、../MU2000/roms の順に探す。

判定は pcm_sha1 の一致。違ったら「どこがどれだけ違うか」を出す。
意図して音を変えたときは `--update` で指紋を焼き直す。
"""
import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fingerprint as fpmod
import make_test_midi

ROOT = Path(__file__).resolve().parent.parent
# 道具の置き場。Makefile の BUILD と同じもの（Linux は build-linux、
# CROSS=windows は build-windows など）。環境変数 SMU_BUILD で渡す
BUILD = Path(os.environ.get("SMU_BUILD") or (ROOT / "build"))
if not BUILD.is_absolute():
    BUILD = ROOT / BUILD
WORK = BUILD / "tests"
BASE = ROOT / "tests"
RATE = 44100

NEEDED = ("mu2000_flash.bin", "dump/xv364a0.ic49")

# The same tools on both platforms; only the suffix differs (make test builds
# with the same name on macOS, see the EXE variable in the Makefile)
EXE = ".exe" if os.name == "nt" else ""


def tool(name):
    return BUILD / (name + EXE)


def find_roms(given):
    cands = []
    if given:
        cands.append(Path(given))
    if os.environ.get("SMU2000_ROMS"):
        cands.append(Path(os.environ["SMU2000_ROMS"]))
    cands += [ROOT / "roms", ROOT.parent / "MU2000" / "roms"]
    for c in cands:
        if all((c / n).exists() for n in NEEDED):
            return c
    return None


def run(cmd, out=None, err=None, env=None):
    """out / err は書き出す先。同じ名前を渡せば 1 つの記録にまとめる。
    env は足す環境変数（渡さなければ今の環境のまま）。
    **呼んだ形を記録の先頭に残す**（後で手で再現できるように）"""
    fo = open(out or os.devnull, "w", encoding="utf-8")
    fo.write("# " + " ".join('"%s"' % c if " " in str(c) else str(c)
                            for c in cmd) + chr(10))
    if env:
        fo.write("# " + " ".join("%s=%s" % kv for kv in env.items()) + chr(10))
    fo.flush()
    ee = None if env is None else {**os.environ, **env}
    fe = fo if (err and err == out) else open(err or os.devnull, "w", encoding="utf-8")
    try:
        return subprocess.run([str(c) for c in cmd], stdout=fo, stderr=fe, env=ee).returncode
    finally:
        fe.close()
        if fe is not fo:
            fo.close()


class Report:
    def __init__(self):
        self.rows = []
        self.bad = 0

    def add(self, name, ok, note=""):
        self.rows.append((name, ok, note))
        if not ok:
            self.bad += 1

    def show(self):
        print()
        print("  結果")
        for name, ok, note in self.rows:
            print("    %-10s %s  %s" % (name, "合" if ok else "×", note))
        print()
        if self.bad:
            print("  %d 件食い違った。意図した変更なら --update で指紋を焼き直す" % self.bad)
        else:
            print("  全部そろっている")


def step_verify(rep, update):
    """ROM 不要。swp30 を素で叩いて、レジスタと乱数が動いているか"""
    exe = tool("verify")
    if not exe.exists():
        rep.add("verify", False, "build/verify%s が無い。make を先に" % EXE)
        return
    got = subprocess.run([str(exe)], capture_output=True, text=True,
                         encoding="utf-8").stdout
    ref = BASE / "verify.txt"
    if update or not ref.exists():
        ref.write_text(got, encoding="utf-8")
        rep.add("verify", True, "焼いた")
        return
    want = ref.read_text(encoding="utf-8")
    if got == want:
        rep.add("verify", True)
    else:
        rep.add("verify", False, "出力が変わった")
        for a, b in zip(want.splitlines(), got.splitlines()):
            if a != b:
                print("    前: %s" % a)
                print("    今: %s" % b)


def step_statetest(rep, roms, midi):
    exe = tool("statetest")
    if not exe.exists():
        rep.add("statetest", False, "build/statetest%s が無い" % EXE)
        return
    # DIN の口と USB の口の両方で確かめる。USB のときしか動かない所（HOST SELECT を
    # 読む 2 つ目の A/D 変換器）の写し忘れは、DIN だけでは見つからない（issue #18）
    ok = True
    note = ""
    # 3 つ目は軽量モード（C++ のエフェクト）。2 台が同時に軽量モードで動く道を通す
    for tag, extra, env in (("statetest", [], None), ("statetest USB", ["--usb"], None),
                            ("statetest 軽量", [], {"SMU2000_NATIVE_FX": "2"})):
        log = WORK / ("statetest%s.log" % ("_usb" if extra else ("_nfx" if env else "")))
        rc = run([exe, roms, midi, "--warm", "2.0", "--steps", "50"] + extra, out=log, err=log, env=env)
        this = ""
        for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("詰めると"):
                this = line
        rep.add(tag, rc == 0, this)
        ok = ok and rc == 0
        note = note or this


# MIDI を流し始める時刻を **固定する**。
#
# render は既定では「firmware が受信を有効にした瞬間」を待ってから流す。
# その瞬間は実測 7.8801 秒だが、**呼び方によって 4 サンプルずれることがある**
# （doc/todo.md「起動の長さが揺れる」）。待たせると指紋がその揺れを拾って
# しまうので、試験では --boot で 8 秒に固定する。8 秒は実測の起動より後。
BOOT_AT = 8.0


def render(roms, name, midi, seconds, extra=(), env=None):
    """鳴らして指紋を作る。(指紋, かかった秒) を返す"""
    wav = WORK / ("%s.wav" % name)
    log = WORK / ("%s.log" % name)
    out = WORK / ("%s.out" % name)
    t0 = time.time()
    rc = run([tool("render"), roms, midi, wav, "%.3f" % seconds,
              "--boot", "%.3f" % BOOT_AT, "-v"] + list(extra), out=out, err=log, env=env)
    took = time.time() - t0
    if rc != 0 or not wav.exists():
        return None, took
    frames, rate, ch, _ = fpmod.load_wav(str(wav))
    boot = int(round(BOOT_AT * rate))
    got = len(frames) // ch - int(round(seconds * rate))
    if got != boot:
        print("  %s: 起動ぶんの長さが %d（8 秒なら %d）" % (name, got, boot))
    fp = fpmod.make(str(wav), str(log), boot, name=name, seconds=seconds)
    return fp, took


def step_cases(rep, roms, cases, update):
    first = None
    for name, (midi, seconds) in cases.items():
        fp, took = render(roms, name, midi, seconds)
        if fp is None:
            rep.add(name, False, "鳴らせなかった（%s.out を見る）" % name)
            continue
        if first is None:
            first = (name, midi, seconds, fp)
        ref = BASE / ("%s.json" % name)
        line = "%s  %.1f 秒" % (fpmod.summary(fp), took)
        if update or not ref.exists():
            ref.write_text(json.dumps(fp, ensure_ascii=False, indent=1) + "\n",
                           encoding="utf-8")
            rep.add(name, True, "焼いた  " + line)
            continue
        old = json.loads(ref.read_text(encoding="utf-8"))
        if old.get("pcm_sha1") == fp["pcm_sha1"]:
            rep.add(name, True, line)
        else:
            rep.add(name, False, line)
            print("  %s が変わった:" % name)
            for l in fpmod.diff(old, fp):
                print(l)
    return first


def step_jit_off(rep, roms, cases):
    """Render every song twice, once with both JITs and once with neither, and
    compare the wav files byte for byte. The JITs are supposed to do exactly
    what the interpreter does, so any difference is a mis-translation (the same
    check the MEG JIT's author asks for by hand). The reference carried by
    tests/*.json cannot catch this on its own, because it is one number."""
    names, bad = [], []
    for name, (midi, seconds) in cases.items():
        on = WORK / ("%s.wav" % name)                     # 3 番が焼いた（JIT あり）
        off = WORK / ("%s_nojit.wav" % name)
        rc = run([tool("render"), roms, midi, off, "%.3f" % seconds,
                  "--boot", "%.3f" % BOOT_AT],
                 out=WORK / ("%s_nojit.out" % name), err=WORK / ("%s_nojit.log" % name),
                 env={"SMU2000_SH2_JIT": "0", "SMU2000_MEG_JIT": "0"})
        if rc != 0 or not off.exists() or not on.exists():
            bad.append(name)
            continue
        if on.read_bytes() != off.read_bytes():
            bad.append(name)
        else:
            names.append(name)
    note = "%d 件とも同じ" % len(names) if not bad else "違う: " + ", ".join(bad)
    rep.add("JIT 入切", not bad, note)


def step_threading(rep, roms, first):
    """1 サンプルの中で 2 個の SWP30 は独立——が崩れていないか"""
    if not first:
        return
    name, midi, seconds, fp = first
    fp2, _ = render(roms, name + "_single", midi, seconds, extra=["--single"])
    if fp2 is None:
        rep.add("別糸", False, "--single で鳴らせなかった")
    else:
        ok = fp2["pcm_sha1"] == fp["pcm_sha1"]
        rep.add("別糸", ok, "%s で threaded と --single が%s" %
                (name, "一致" if ok else "食い違う"))



def step_native_engine(rep, roms, cases):
    """**firmware を走らせない口**（doc/native-engine.md の段 2）が、既定の道と
    同じ大きさで鳴るか。1 音ずつの波形までは合わないので、大きさ（rms）と
    低域比で見る。写し取りはこの試験の中では残さない（SMU2000_NO_VOICECACHE）"""
    import math
    env = {"SMU2000_NO_VOICECACHE": "1"}   # 試験は毎回まっさらから
    worst = 0.0
    worst_name = ""
    bad = []
    for name, (midi, seconds) in cases.items():
        base = BASE / ("%s.json" % name)
        if not base.exists():
            continue
        ref = json.loads(base.read_text(encoding="utf-8"))
        fp, _ = render(roms, name + "_ne", midi, seconds,
                       extra=["--native-engine"], env=env)
        if fp is None:
            bad.append("%s: 鳴らせなかった" % name)
            continue
        a = max(ref["rms"])
        b = max(fp["rms"])
        if a <= 1.0 or b <= 1.0:
            bad.append("%s: 音が無い" % name)
            continue
        d = 20.0 * math.log10(b / a)
        if abs(d) > abs(worst):
            worst, worst_name = d, name
        if abs(d) > 1.5:
            bad.append("%s %+.2f dB" % (name, d))
    ok = not bad
    note = ("いちばん違ったのは %s の %+.2f dB" % (worst_name, worst)) if ok \
        else "、".join(bad)
    rep.add("native の口", ok, note)

def step_xg(rep, roms):
    """定義表の番地・大きさ・範囲が firmware と合っているか。音は見ない"""
    exe = tool("xgtest")
    if not exe.exists():
        rep.add("xg", False, "build/xgtest%s が無い" % EXE)
        return
    log = WORK / "xgtest.log"
    rc = run([exe, roms], out=log, err=log)
    lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
    head = [l for l in lines if l.startswith("書いて読み返す")]
    note = head[0] if head else ""
    if rc != 0:
        note += "（build/tests/xgtest.log）"
        for l in lines:
            if l.strip().startswith("NG"):
                print("   " + l.strip())
    rep.add("xg", rc == 0, note)


def step_sampling(rep, roms):
    """パネルで録音して試聴し、録った 440Hz が返ってくるか"""
    # The name the Makefile builds: samptest.exe on Windows, samptest on macOS
    exe = BUILD / ("samptest" + EXE)
    if not exe.exists():
        rep.add("sampling", False, "%s が無い" % exe)
        return
    log = WORK / "samptest.log"
    rc = run([exe, roms], out=log, err=log)
    lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
    head = [l for l in lines if l.startswith("サンプリング:")]
    note = head[0] if head else ""
    if rc != 0:
        note += "（build/tests/samptest.log）"
        for l in lines:
            if l.startswith("NG"):
                print("   " + l.strip())
    rep.add("sampling", rc == 0, note)


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser()
    ap.add_argument("--roms")
    ap.add_argument("--only", help="この名前の鳴らし比べだけ")
    ap.add_argument("--update", action="store_true", help="指紋を焼き直す")
    ap.add_argument("--require-roms", action="store_true",
                    help="ROM が無ければ失敗にする")
    a = ap.parse_args()

    WORK.mkdir(parents=True, exist_ok=True)
    BASE.mkdir(parents=True, exist_ok=True)
    rep = Report()

    print("== 1. verify（ROM 不要）")
    step_verify(rep, a.update)

    roms = find_roms(a.roms)
    if roms is None:
        print()
        print("ROM が見つからないので、音の試験は飛ばす。")
        print("  探した場所: --roms / SMU2000_ROMS / roms / ../MU2000/roms")
        print("  要るもの: " + " ".join(NEEDED))
        rep.show()
        return 1 if (rep.bad or a.require_roms) else 0
    print("   ROM: %s" % roms)

    cases = {}
    for name, (path, seconds) in make_test_midi.build(WORK).items():
        if not a.only or a.only == name:
            cases[name] = (path, seconds)
    if not cases:
        print("その名前の試験は無い: %s" % a.only)
        return 1

    print()
    print("== 2. statetest")
    step_statetest(rep, roms, next(iter(cases.values()))[0])

    print()
    print("== 3. 鳴らし比べ（%d 件）" % len(cases))
    first = step_cases(rep, roms, cases, a.update)

    print()
    print("== 3b. native の口（SH-2 を止めて鳴らす）")
    step_native_engine(rep, roms, cases)

    print()
    print("== 4. JIT あり・なしで wav がバイト単位で同じか")
    step_jit_off(rep, roms, cases)

    print()
    print("== 5. スレーブを別の糸で回しても同じ音か")
    step_threading(rep, roms, first)

    if not a.only:
        print()
        print("== 6. パラメータの層を firmware に読み返させる")
        step_xg(rep, roms)

        print()
        print("== 7. サンプリング（録音して試聴する）")
        step_sampling(rep, roms)

    rep.show()
    return 1 if rep.bad else 0


if __name__ == "__main__":
    sys.exit(main())
