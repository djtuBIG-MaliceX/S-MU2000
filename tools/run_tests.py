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
import shutil
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


# **いくつ同時に鳴らすか**。曲はどれも別々の render で鳴らすので、互いに関係が無い
# （出てくる音は時計でなく機械の中の時刻で決まるので、同時に回しても 1 ビットも変わらない）。
# render は 1 本で 2 つの糸（2 個目の SWP30）を使うので、既定はコア数の半分。
# `-j` か環境変数 SMU_JOBS で変える（1 なら前と同じく 1 本ずつ）
JOBS = max(1, (os.cpu_count() or 2) // 2)


def pmap(fn, items):
    """items の 1 つずつに fn を JOBS 本まで同時に掛け、**元の並びで**結果を返す。
    fn は外の exe を待つだけなので、糸で足りる（Python の錠は待っている間は外れている）"""
    items = list(items)
    if JOBS <= 1 or len(items) <= 1:
        return [fn(x) for x in items]
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=JOBS) as ex:
        return list(ex.map(fn, items))


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

    def say(self, line):
        """試験の途中の 1 行（NG の中身など）"""
        print(line)

    def merge(self, other):
        self.rows += other.rows
        self.bad += other.bad

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


class Buffered(Report):
    """同時に回す段のための控え。結果の行も途中の行も溜めておき、段の順に後で出す"""
    def __init__(self):
        super().__init__()
        self.lines = []

    def say(self, line):
        self.lines.append(line)


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
    done = pmap(lambda kv: render(roms, kv[0], kv[1][0], kv[1][1]), cases.items())
    for (name, (midi, seconds)), (fp, took) in zip(cases.items(), done):
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

    def nojit(kv):
        name, (midi, seconds) = kv
        return run([tool("render"), roms, midi, WORK / ("%s_nojit.wav" % name), "%.3f" % seconds,
                    "--boot", "%.3f" % BOOT_AT],
                   out=WORK / ("%s_nojit.out" % name), err=WORK / ("%s_nojit.log" % name),
                   env={"SMU2000_SH2_JIT": "0", "SMU2000_MEG_JIT": "0"})

    rcs = pmap(nojit, cases.items())
    for (name, (midi, seconds)), rc in zip(cases.items(), rcs):
        on = WORK / ("%s.wav" % name)                     # 3 番が焼いた（JIT あり）
        off = WORK / ("%s_nojit.wav" % name)
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



# **波形の相関の下限**（試験ごと）。いま出ている値から少し余裕を引いたもの。
# ここを下回ったら落ちる ＝ 形が崩れたら気づける。
# dense がまだ低いのは分かっている不具合（写し取りの音だけ、実機の側が
# 混み具合で遅れる。doc/native-engine.md の 6.90）
# **1 秒窓の相関の最小の下限**（6.193）。上の SHAPE_MIN は中央なので、
# 一部の窓だけ壊れていても通ってしまう。書いていない試験は 0.80。
#
# `assign` が低いのは**分かっている穴**。つまみの割り当て（6 つ組）の
# うち、**LFO のフィルタ変調の深さだけ**しか native は鳴らせない。
# 残り（音程・切る高さ・音量・LFO の音程/音量）は firmware に任せるので、
# **割り当てを外したあとにツマミを動かすと、鳴っている音に掛からない**
#（doc/native-engine.md の 6.193）
SHAPE_LOW = {
    # 分かっている穴。つまみの割り当て（6 つ組）のうち、音程・音量・
    # **LFO のフィルタ変調の深さだけ**しか native は鳴らせないので、
    # 割り当てを外したあとにつまみを動かすと、**すでに鳴っている音に
    # 掛からない**（測値 50%。doc/native-engine.md の 6.193）
    "assign":  0.65,
    "bendasn": 0.65,
    "progrel": 0.65,
    "banklsb": 0.65,
    "velsens": 0.65,
    "fenvcc": 0.65,
    # **離した音の尾**が残っている（6.206）。音程（`0x11`）は
    # サンプル＆ホールドも含めて**18 本とも実機と同じ値・同じ時刻**
    # になったが、実機がスロットを空けたあともこちらが
    # 切る高さを動かし続けるので、尾の音色がずれる（測値 24%）
    "pegcc": 0.20,
    # 写し取りの音（実機が鳴らす 1 音目）だけ、実機の側が混み具合で
    # 遅れる（6.90・6.122）。測値 62%
    "dense":   0.55,
    # クラッシュの 1 打目も写し取りなので同じ（測値 -12%）。
    # 音が雑音に近いので、数サンプルずれるだけで相関が負になる。
    # 2 サンプルずらすと残差は 6% まで落ちる
    "althh":  -0.30,
    # 同じくドラムの雑音。**1 サンプルずらすだけで残差 0.1%**なのに
    # 相関は 42% まで落ちる（打鍵の ±1 は利用者が妥協してよいと決めた範囲）
    "drumrcv": 0.35,
}

SHAPE_MIN = {
    "piano":   0.98, "chord":  0.95, "drums": 0.95, "effects": 0.98,
    "dense":   0.55, "port_b": 0.98, "bend":  0.98, "lofi":    0.98,
    "egcc":    0.98, "porta":  0.95, "at":    0.95, "sxparam": 0.95,
    "assign":  0.95,
    "bendasn": 0.95,
    "progrel": 0.95,
    "banklsb": 0.95,
    "velsens": 0.95,
    "fenvcc": 0.95,
    "pegcc": 0.95,
    "pedals":  0.95, "partsx": 0.95, "rpn": 0.95, "mono": 0.95,
    # 一晩で足した軸（6.125-6.139）。どれも中央 98-100% 出ている
    "ctlreset": 0.95, "ports": 0.95, "scale": 0.95, "kits": 0.95,
    "ins2": 0.95, "progchg": 0.98, "running": 0.95, "pat": 0.95,
    "ccramp": 0.95, "midreset": 0.98, "partmode": 0.95,
    "drumnrpn": 0.95, "retrig": 0.95, "pedretrig": 0.98, "edges": 0.95,
    "fxchange": 0.95, "dialloop": 0.95, "panrnd": 0.95,
    # 長く伸ばす音（遅れて掛かるビブラート。6.175）
    "longtone": 0.95,
    # 離した音の尾（6.207）
    "reltail": 0.95,
    # まだ触っていなかった CC（68・69・96/97・124/125）
    "ctlrest": 0.95,
    # XG のパート番地のうち、SysEx を 1 度も通していなかった軸
    "xgpeg": 0.95,
    "xghpf": 0.95,
    "xgpegatk": 0.95,
    "xgmwvib": 0.95,
    "xgsys": 0.95,
    # 短い音で写し取ったあとの、遅れて掛かるビブラート（6.217）
    "xgvibshort": 0.95,
    # meter は 15 パートを同時に鳴らすので dense と同じ事情で形が落ちる
    # （狙いは液晶のほうなので、音は緩めに見る）
    "meter": 0.90, "filtcc": 0.95, "keyrange": 0.95, "rcvch": 0.95, "althh": 0.95, "drumrcv": 0.95,
    # keylevel は鍵と強さで音量が大きく動く音色ばかりなので、鍵を押す時刻の
    # ばらつき（6.90）が相関に出やすい。**音量のほうは `native の口` が見る**。
    # 音 1 つずつは tools/native/notelevel.py で見られる
    "keylevel": 0.95,
}


def step_native_engine(rep, roms, cases):
    """**firmware を走らせない口**（doc/native-engine.md の段 2）が、既定の道と
    同じ大きさで鳴るか。1 音ずつの波形までは合わないので、大きさ（rms）と
    低域比で見る。写し取りはこの試験の中では残さない（SMU2000_NO_VOICECACHE）"""
    import math
    env = {"SMU2000_NO_VOICECACHE": "1"}   # 試験は毎回まっさらから
    worst = 0.0
    worst_name = ""
    bad = []
    todo = [(name, v) for name, v in cases.items() if (BASE / ("%s.json" % name)).exists()]
    done = pmap(lambda kv: render(roms, kv[0] + "_ne", kv[1][0], kv[1][1],
                                  extra=["--native-engine"], env=env), todo)
    for (name, (midi, seconds)), (fp, _) in zip(todo, done):
        ref = json.loads((BASE / ("%s.json" % name)).read_text(encoding="utf-8"))
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
    step_native_shape(rep, cases)


def step_native_shape(rep, cases):
    """**波形そのもの**が既定の道と合っているか。大きさ（rms）だけ見ていると、
    音程の包絡線が丸ごと抜けていても気づけなかった（doc の 6.80）。
    1 秒ごとに相関を取り、そのいちばん悪いものを見る"""
    import math
    worst = 2.0
    worst_name = ""
    bad = []
    for name in cases:
        wa = WORK / ("%s.wav" % name)
        wb = WORK / ("%s_ne.wav" % name)
        if not wa.exists() or not wb.exists():
            continue
        fa, ra, ca, _ = fpmod.load_wav(str(wa))
        fb, rb, cb, _ = fpmod.load_wav(str(wb))
        n = min(len(fa) // ca, len(fb) // cb)
        skip = int(round(BOOT_AT * ra))
        cs = []
        for s0 in range(skip, n - ra, ra):
            sa = fa[s0 * ca:(s0 + ra) * ca:ca]
            sb = fb[s0 * cb:(s0 + ra) * cb:cb]
            na = sum(float(x) * x for x in sa)
            nb = sum(float(x) * x for x in sb)
            if na < 1e4 or nb < 1e4:
                continue
            num = sum(float(x) * float(y) for x, y in zip(sa, sb))
            cs.append(num / math.sqrt(na * nb))
        if not cs:
            continue
        med = sorted(cs)[len(cs) // 2]
        low = min(cs)
        if os.environ.get('SHAPE_VERBOSE'):
            print('    %-10s 波形の相関 中央 %.0f%% 最小 %.0f%%'
                  % (name, 100 * med, 100 * low))
        if med < worst:
            worst, worst_name = med, name
        if med < SHAPE_MIN.get(name, 0.9):
            bad.append("%s 中央 %.0f%%" % (name, 100 * med))
        # **最小も見る**（6.193）。中央だけだと、**一部の窓だけ
        # 壊れていても気づけない**。`assign` は中央 100%・最小 50% で、
        # 中央だけの判定をすり抜けていた
        elif low < SHAPE_LOW.get(name, 0.80):
            bad.append("%s 最小 %.0f%%" % (name, 100 * low))
    if worst > 1.5:
        rep.add("native の形", True, "測れなかった")
        return
    ok = not bad
    note = ("いちばん低いのは %s の %.0f%%" % (worst_name, 100 * worst)) if ok \
        else "、".join(bad) + "（下限を割った）"
    rep.add("native の形", ok, note)

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
                rep.say("   " + l.strip())
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
                rep.say("   " + l.strip())
    rep.add("sampling", rc == 0, note)


def step_warm(rep, roms, cases):
    """**2 回目以降の音**（写し取りが済んだ状態）。
    `dense` の相関が 57% で止まっているのは、60 声のうち半分が
    **写し取りの音（実機が鳴らす音）**で、firmware の混み具合が
    firmware の道と違うため（doc/native-engine.md の 6.117・6.121）。
    写し取りが済めばその音も native が鳴らすので、実際に使うときの値は
    こちらになる。1 回鳴らして写しを貯め、2 回目を比べる"""
    import math
    home = WORK / "warmhome"
    shutil.rmtree(home / "S-MU2000" / "voicecal", ignore_errors=True)
    home.mkdir(parents=True, exist_ok=True)
    env = {"LOCALAPPDATA": str(home), "XDG_DATA_HOME": str(home),
           "HOME": str(home)}
    extra = ["--native-engine", "--voicecache"]
    # dense … 写し取りの音がいちばん効く曲、porta … 10ms 格子の位相を使う曲
    # （写し取りが無いと位相が学べず、滑りが前の道に落ちていた。6.145）
    floor = {"dense": 0.70, "porta": 0.95}
    notes, bad = [], []
    for name in ("dense", "porta"):
        if name not in cases:
            continue
        midi, seconds = cases[name]
        if render(roms, "warm1", midi, seconds, extra=extra, env=env)[0] is None:
            bad.append("%s: 1 回目が鳴らせなかった" % name)
            continue
        if render(roms, "warm2", midi, seconds, extra=extra, env=env)[0] is None:
            bad.append("%s: 2 回目が鳴らせなかった" % name)
            continue
        base = WORK / ("%s.wav" % name)
        if not base.exists():
            continue
        fa, ra, ca, _ = fpmod.load_wav(str(base))
        fb, _, cb, _ = fpmod.load_wav(str(WORK / "warm2.wav"))
        n = min(len(fa) // ca, len(fb) // cb)
        skip = int(round(BOOT_AT * ra))
        cs = []
        for s0 in range(skip, n - ra, ra):
            sa = fa[s0 * ca:(s0 + ra) * ca:ca]
            sb = fb[s0 * cb:(s0 + ra) * cb:cb]
            na = sum(float(x) * x for x in sa)
            nb = sum(float(x) * x for x in sb)
            if na < 1e4 or nb < 1e4:
                continue
            num = sum(float(x) * float(y) for x, y in zip(sa, sb))
            cs.append(num / math.sqrt(na * nb))
        if not cs:
            bad.append("%s: 音が無い" % name)
            continue
        med = sorted(cs)[len(cs) // 2]
        notes.append("%s %.0f%%" % (name, 100 * med))
        if med < floor.get(name, 0.9):
            bad.append("%s %.0f%%" % (name, 100 * med))
    rep.add("2 回目", not bad,
            "、".join(bad or notes) + ("（下限を割った）" if bad else ""))


def step_usb(rep, roms, cases):
    """**USB の口でも native が firmware と同じ時刻で鳴るか**
    （doc/native-engine.md の 6.120）。プラグインは USB が既定なのに、
    native の口は MIDI のバイトを DIN の速さ（31250 baud ＝ 14.1 サンプル）で
    並べていて、実機（19500 byte/s ＝ 2.26 サンプル）より 1 音あたり
    37 サンプル遅れていた。試験はふだん DIN で鳴らすので気づけなかった"""
    import math
    env = {"SMU2000_NO_VOICECACHE": "1"}
    notes, bad = [], []
    # chord … USB のバイトの速さ、ports … 4 つの口（C と D は USB だけ）
    for name in ("chord", "ports"):
        if name not in cases:
            continue
        midi, seconds = cases[name]
        a, _ = render(roms, "usb_fw", midi, seconds, extra=["--usb"], env=env)
        b, _ = render(roms, "usb_ne", midi, seconds,
                      extra=["--usb", "--native-engine"], env=env)
        if a is None or b is None:
            bad.append("%s: 鳴らせなかった" % name)
            continue
        fa, ra, ca, _ = fpmod.load_wav(str(WORK / "usb_fw.wav"))
        fb, rb, cb, _ = fpmod.load_wav(str(WORK / "usb_ne.wav"))
        n = min(len(fa) // ca, len(fb) // cb)
        skip = int(round(BOOT_AT * ra))
        cs = []
        for s0 in range(skip, n - ra, ra):
            sa = fa[s0 * ca:(s0 + ra) * ca:ca]
            sb = fb[s0 * cb:(s0 + ra) * cb:cb]
            na = sum(float(x) * x for x in sa)
            nb = sum(float(x) * x for x in sb)
            if na < 1e4 or nb < 1e4:
                continue
            num = sum(float(x) * float(y) for x, y in zip(sa, sb))
            cs.append(num / math.sqrt(na * nb))
        if not cs:
            bad.append("%s: 音が無い" % name)
            continue
        med = sorted(cs)[len(cs) // 2]
        notes.append("%s %.0f%%" % (name, 100 * med))
        # ports は USB のとき、実機の側が**口ごとに違う遅れ**で鳴らす
        # （口 A +43 に対し B +117・C +151・D +104 サンプル。まだ真似できて
        # いない。doc/native-engine.md の 6.126）。DIN では 100% 出る
        if med < (0.80 if name == "ports" else 0.95):
            bad.append("%s %.0f%%" % (name, 100 * med))
    rep.add("USB の口", not bad, "、".join(bad or notes) + ("（下限を割った）" if bad else ""))


def step_meter(rep, roms):
    """**液晶のメーター**（doc/native-engine.md の 6.148）。firmware の道と
    native の口で同じ曲を鳴らして、**棒の字が並ぶ 16 マス**を突き合わせる。
    メーターの目盛りは「強さ x パートの目盛り / 128」で、実機との差は 1 以内。
    点の境目をまたぐと 1 マスだけずれることがあるので、2 マスまで許す"""
    exe = tool("render")
    mid = WORK / "meter.mid"
    if not exe.exists() or not mid.exists():
        rep.add("メーター", True, "この回では見ない")
        return
    got = {}
    for tag, extra in (("fw", []), ("ne", ["--native-engine"])):
        log = WORK / ("meter_%s.log" % tag)
        rc = run([exe, roms, mid, WORK / ("meter_%s.wav" % tag), "5",
                  "--boot", "%.3f" % BOOT_AT, "--lcd-at", "3.4"] + extra,
                 out=log, err=log, env={"SMU2000_NO_VOICECACHE": "1"})
        if rc != 0:
            rep.add("メーター", False, "%s で鳴らせなかった" % tag)
            return
        hit = [l for l in log.read_text(encoding="utf-8", errors="replace").splitlines()
               if l.startswith("LCDHEX")]
        if not hit:
            rep.add("メーター", False, "%s の液晶が読めなかった" % tag)
            return
        v = hit[0].split()[1:]
        # 上の行の 1-8 桁目と下の行の 1-8 桁目
        got[tag] = [v[1 + i] for i in range(8)] + [v[24 + 1 + i] for i in range(8)]
    if all(x == "89" for x in got["fw"][8:]):
        rep.add("メーター", False, "firmware の道で棒が動いていない")
        return
    if all(x == "89" for x in got["ne"][8:]):
        rep.add("メーター", False, "native の口で棒が動かない")
        return
    bad = [i for i in range(16) if got["fw"][i] != got["ne"][i]]
    ok = len(bad) <= 2
    note = "16 マス中 %d マスが同じ" % (16 - len(bad))
    if bad:
        note += "（%s / %s）" % (" ".join(got["fw"]), " ".join(got["ne"]))
    rep.add("メーター", ok, note)


def step_screen(rep, roms):
    """**演奏画面の音色まわり**（doc/native-engine.md の 6.190）。firmware の道と
    native の口で同じ曲を鳴らして、**音色名（行 0 の 9-16）・プログラムの 3 桁
    （行 1 の 14-16）・**バンクの 3 桁**（行 1 の 10-12。6.202）・楽器の絵（外字 0-2 と 4-6）**を 0.1 秒ごとに突き合わせる。
    native はこれを自分で描くので、1 マスでも違えば読み違えている"""
    exe = tool("render")
    mid = WORK / "progchg.mid"
    if not exe.exists() or not mid.exists():
        rep.add("演奏画面", True, "この回では見ない")
        return
    got = {}
    for tag, extra in (("fw", []), ("ne", ["--native-engine"])):
        log = WORK / ("screen_%s.log" % tag)
        rc = run([exe, roms, mid, WORK / ("screen_%s.wav" % tag), "7",
                  "--boot", "%.3f" % BOOT_AT, "--lcd-every", "0.1"] + extra,
                 out=log, err=log, env={"SMU2000_NO_VOICECACHE": "1"})
        if rc != 0:
            rep.add("演奏画面", False, "%s で鳴らせなかった" % tag)
            return
        d = {}
        for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
            f = line.split()
            if f and f[0] == "LCD" and len(f) >= 50:
                d.setdefault(f[1], {})["d"] = f[2:50]
            elif f and f[0] == "CG" and len(f) >= 66:
                d.setdefault(f[1], {})["c"] = f[2:66]
        got[tag] = d
    ts = sorted(set(got["fw"]) & set(got["ne"]), key=float)
    if len(ts) < 10:
        rep.add("演奏画面", False, "液晶が読めなかった")
        return
    ICON = list(range(0, 24)) + list(range(32, 56))

    def fields(v):
        return (v["d"][9:17], v["d"][24 + 14:24 + 17],
                v["d"][24 + 10:24 + 13], [v["c"][i] for i in ICON])

    # **native が先を行くのは許す**。native は MIDI を受けた 25ms 後に描き、
    # firmware は 100ms につき 5ms しか回らないので最大 100ms 遅れる。
    # だから「その時刻か、そのあと 0.3 秒のどれかの firmware の絵と同じ」なら合格。
    # 起動の直後（1.2 秒まで）は firmware がまだ絵を描いていないので見ない
    names = set()
    bad = []
    for k, t in enumerate(ts):
        if float(t) < 1.2:
            continue
        b = got["ne"][t]
        if "d" not in b or "c" not in b:
            continue
        fb = fields(b)
        ahead = []
        for t2 in ts[k:k + 4]:
            a = got["fw"][t2]
            if "d" in a and "c" in a:
                ahead.append(fields(a))
        if not ahead:
            continue
        names.add(tuple(ahead[0][0]))
        if fb not in ahead:
            bad.append((t, ahead[0], fb))
    if len(names) < 2:
        rep.add("演奏画面", False, "音色名がひとつも替わっていない")
        return
    ok = not bad
    note = "%d 点 × 音色名 %d 通りが firmware と同じ" % (len(ts), len(names))
    if bad:
        t, fa, fb = bad[0]
        note = "%d 点中 %d 点が違う（%s 秒: %s / %s）" % (
            len(ts), len(bad), t,
            "".join(chr(int(x, 16)) if 0x20 <= int(x, 16) < 0x7f else "."
                    for x in fa[0] + fa[1]),
            "".join(chr(int(x, 16)) if 0x20 <= int(x, 16) < 0x7f else "."
                    for x in fb[0] + fb[1]))
    rep.add("演奏画面", ok, note)


def step_dial(rep, roms, cases):
    """**パネルのダイヤルで音色を替える**（doc/native-engine.md の 6.146）。
    ジョグダイヤルの音色替えは MIDI を通らないので、native が拾えないと
    「画面は変わるのに音が変わらない」。鳴らしている最中に 4 目盛り回して、
    液晶と波形を firmware の道と突き合わせる"""
    import math
    exe = BUILD / ("panel" + EXE)
    mid = WORK / "dialloop.mid"
    if not exe.exists() or not mid.exists():
        rep.add("ダイヤル", True, "この回では見ない")
        return
    lcd, wav = {}, {}
    for tag, extra in (("fw", []), ("ne", ["--native"])):
        out = WORK / ("dial_%s.wav" % tag)
        log = WORK / ("dial_%s.log" % tag)
        rc = run([exe, roms, "--keys", "play", "--mid", mid, "10",
                  "--turn-at", "3.0", "4", "--wav", out] + extra, out=log, err=log)
        if rc != 0 or not out.exists():
            rep.add("ダイヤル", False, "%s で鳴らせなかった" % tag)
            return
        txt = log.read_text(encoding="utf-8", errors="replace").splitlines()
        hit = [l for l in txt if l.startswith("  0 |")]
        lcd[tag] = hit[0] if hit else ""
        wav[tag] = out
    if lcd["fw"] != lcd["ne"]:
        rep.add("ダイヤル", False,
                "液晶が違う: %s / %s" % (lcd["fw"].strip(), lcd["ne"].strip()))
        return
    fa, ra, ca, _ = fpmod.load_wav(str(wav["fw"]))
    fb, _, cb, _ = fpmod.load_wav(str(wav["ne"]))
    n = min(len(fa) // ca, len(fb) // cb)
    cs = []
    for s0 in range(0, n - ra, ra):
        sa = fa[s0 * ca:(s0 + ra) * ca:ca]
        sb = fb[s0 * cb:(s0 + ra) * cb:cb]
        na = sum(float(x) * x for x in sa)
        nb = sum(float(x) * x for x in sb)
        if na < 1e3 or nb < 1e3:
            continue
        num = sum(float(x) * float(y) for x, y in zip(sa, sb))
        cs.append(num / math.sqrt(na * nb))
    if not cs:
        rep.add("ダイヤル", False, "音が無い")
        return
    med = sorted(cs)[len(cs) // 2]
    ok = med >= 0.95
    rep.add("ダイヤル", ok, "音色が替わって波形の相関 %.0f%%" % (100 * med))


# パネルの試験で押すボタン（品書きを一巡りする）
PANEL_KEYS = ("play,util,enter,value+,value+,exit,edit,enter,value+,exit,exit,"
              "part+,mute,play,drum,piano,organ,select,edit,enter,enter,exit,exit")


def step_panel(rep, roms):
    """**native の口でもパネルが効くか**（doc/native-engine.md の 6.119）。
    ボタン・ダイヤル・液晶はぜんぶ firmware の仕事なので、firmware を細く
    回したままだと一切効かない。同じボタンの並びを firmware の道と native の
    口で押して、液晶が 1 行残らず同じになるかを見る"""
    exe = BUILD / ("panel" + EXE)
    if not exe.exists():
        rep.add("パネル", False, "%s が無い" % exe)
        return
    outs = []
    for tag, extra in (("fw", []), ("ne", ["--native"])):
        log = WORK / ("panel_%s.log" % tag)
        rc = run([exe, roms, "--keys", PANEL_KEYS, "--trace"] + extra,
                 out=log, err=log)
        if rc != 0:
            rep.add("パネル", False, "%s で鳴らせなかった" % tag)
            return
        txt = log.read_text(encoding="utf-8", errors="replace").splitlines()
        # `--trace` が出す「ボタン名 + 液晶 1 行」だけを取る
        outs.append([l for l in txt if l.startswith("  ") and "|" in l])
    if not outs[0]:
        rep.add("パネル", False, "液晶が読めなかった")
        return
    bad = [i for i in range(min(len(outs[0]), len(outs[1])))
           if outs[0][i] != outs[1][i]]
    ok = not bad and len(outs[0]) == len(outs[1])
    if ok:
        note = "%d 行とも firmware と同じ" % len(outs[0])
    elif bad:
        note = "%d 行目から違う: %s / %s" % (bad[0] + 1,
                                             outs[0][bad[0]].strip(),
                                             outs[1][bad[0]].strip())
    else:
        note = "行数が違う（%d / %d）" % (len(outs[0]), len(outs[1]))
    rep.add("パネル", ok, note)


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser()
    ap.add_argument("--roms")
    ap.add_argument("--only", help="この名前の鳴らし比べだけ")
    ap.add_argument("--update", action="store_true", help="指紋を焼き直す")
    ap.add_argument("--require-roms", action="store_true",
                    help="ROM が無ければ失敗にする")
    ap.add_argument("-j", "--jobs", type=int,
                    help="同時に鳴らす数（既定はコア数の半分。SMU_JOBS でも渡せる）")
    a = ap.parse_args()
    global JOBS
    if a.jobs or os.environ.get("SMU_JOBS"):
        JOBS = max(1, a.jobs or int(os.environ["SMU_JOBS"]))

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
    # **5-10 は同時に回す**。どれも 1-2 本の render か道具を順に回すだけで、作業の
    # ファイルの名前も段ごとに別。1 本ずつだと合わせて 76 秒、同時なら長い段 1 つぶん。
    # 結果と途中の行は段ごとに控えておき、下で段の順に出す
    steps = [("== 5. スレーブを別の糸で回しても同じ音か", lambda r: step_threading(r, roms, first))]
    if not a.only:
        steps += [
            ("== 6. パラメータの層を firmware に読み返させる", lambda r: step_xg(r, roms)),
            ("== 7. サンプリング（録音して試聴する）", lambda r: step_sampling(r, roms)),
            ("== 8. パネル（native の口でもボタンと液晶が効くか）",
             lambda r: (step_panel(r, roms), step_meter(r, roms), step_screen(r, roms),
                        step_dial(r, roms, cases))),
            ("== 9. USB の口（プラグインの既定）", lambda r: step_usb(r, roms, cases)),
            ("== 10. 2 回目の音（写し取りが済んだ状態）", lambda r: step_warm(r, roms, cases)),
        ]
    subs = [Buffered() for _ in steps]
    if JOBS <= 1:
        for (_, fn), sub in zip(steps, subs):
            fn(sub)
    else:
        from concurrent.futures import ThreadPoolExecutor
        with ThreadPoolExecutor(max_workers=len(steps)) as ex:
            for f in [ex.submit(fn, sub) for (_, fn), sub in zip(steps, subs)]:
                f.result()
    for i, ((title, _), sub) in enumerate(zip(steps, subs)):
        if i:
            print()
        print(title)
        for line in sub.lines:
            print(line)
        rep.merge(sub)

    rep.show()
    return 1 if rep.bad else 0


if __name__ == "__main__":
    sys.exit(main())
