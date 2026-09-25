// license:BSD-3-Clause
//
// S-MU2000: WebAssembly バイナリの小型エミッタ（MEG JIT 専用、swp30_jit.cpp から使う）。
//
// 「(i32,i32,i32)->void」の関数 1 本と「env.lfo (i32,i32)->i32」のインポート 1 個だけ
// 出すので、セクションは type / import / function / export / code の 5 つ。テーブルは使わない。
// 命令は MEG JIT が実際に使うものだけ（足し算・掛け算・論理・シフト・読み書き・分岐・call）。

#ifndef SWP30_WASMASM_H
#define SWP30_WASMASM_H

#include "../../compat/mamecompat.h"

#include <cstring>
#include <vector>

namespace wasmasm {

enum valtype : u8 { i32_t = 0x7F, i64_t = 0x7E };

inline void put_uleb(std::vector<u8> &v, u64 x)
{
	do {
		u8 b = u8(x & 0x7f);
		x >>= 7;
		if (x)
			b |= 0x80;
		v.push_back(b);
	} while (x);
}

inline void put_sleb(std::vector<u8> &v, s64 x)
{
	for (;;) {
		u8 b = u8(x & 0x7f);
		x >>= 7;                                 // 算術右シフト（s64）
		const bool done = (x == 0 && !(b & 0x40)) || (x == -1 && (b & 0x40));
		if (done) { v.push_back(b); return; }
		v.push_back(b | 0x80);
	}
}

class emitter {
public:
	// ---- 局所変数 ----
	// 引き数 3 つ（ms, swp, ram）が自动で局所 0-2。add_local() がその次の番号を返す
	u32 add_local(u8 type)
	{
		m_local_types.push_back(type);
		return u32(m_local_types.size()) + 2;
	}

	// st* の積み替えと select クランプが使う一時局所。ビルド側で
	// i32 と i64 を 1 個ずつ add_local して渡す（渡さないと局所 0 を壊す）
	void set_scratch(u32 l32, u32 l64) { m_scr32 = l32; m_scr64 = l64; }

	// ---- 命令 ----
	void raw(u8 op) { m_code.push_back(op); }
	void uleb(u64 x) { put_uleb(m_code, x); }
	void sleb(s64 x) { put_sleb(m_code, x); }

	void i32_const(s32 v) { raw(0x41); sleb(v); }
	void i64_const(s64 v) { raw(0x42); sleb(v); }

	void get(u32 i) { raw(0x20); uleb(i); }
	void set(u32 i) { raw(0x21); uleb(i); }
	void tee(u32 i) { raw(0x22); uleb(i); }

	// 読み書き。命令 byte より前に引き数（基盤）を置かねばならん（コードの並びが実行順）。
	// オフセットは構造体内の位置（ビルド時に確定）。
	// wasm の store が取る並びは [アドレス, 値]（値が上）。呼び出し側は「値を積んだ」
	// ところまでなので、一時的な局所（set_scratch で渡す）で受け直してから
	// 基盤 → 値の順に積み直す。st*_at は [ea, 値] がalready 積んである前提（素の命令 byte）
	void ld32(u32 base, s32 off) { get(base); raw(0x28); uleb(2); sleb(off); }
	void ld8u(u32 base, s32 off) { get(base); raw(0x2D); uleb(0); sleb(off); }
	void ld8s(u32 base, s32 off) { get(base); raw(0x2C); uleb(0); sleb(off); }
	void ld16u(u32 base, s32 off) { get(base); raw(0x2F); uleb(1); sleb(off); }
	void ld16s(u32 base, s32 off) { get(base); raw(0x2E); uleb(1); sleb(off); }
	void ld64(u32 base, s32 off) { get(base); raw(0x29); uleb(3); sleb(off); }
	void st32(u32 base, s32 off) { set(m_scr32); get(base); get(m_scr32); raw(0x36); uleb(2); sleb(off); }
	void st8(u32 base, s32 off) { set(m_scr32); get(base); get(m_scr32); raw(0x3A); uleb(0); sleb(off); }
	void st16(u32 base, s32 off) { set(m_scr32); get(base); get(m_scr32); raw(0x3B); uleb(1); sleb(off); }
	void st64(u32 base, s32 off) { set(m_scr64); get(base); get(m_scr64); raw(0x37); uleb(3); sleb(off); }
	// 基盤がストップの足し算あと（m[枠の番号] の間接先）。[ea, v] の順で積んである
	void st32_at() { raw(0x36); uleb(2); sleb(0); }
	// 読みも計算アドレス版（[ea] を積んでから）
	void ld16u_at() { raw(0x2F); uleb(1); sleb(0); }
	void st16_at() { raw(0x3B); uleb(1); sleb(0); }

	// i32 演算（スタック型: 積んだ順に a, b。i32_sub は a - b）
	void i32_add() { raw(0x6A); } void i32_sub() { raw(0x6B); } void i32_mul() { raw(0x6C); }
	void i32_and() { raw(0x71); } void i32_or() { raw(0x72); } void i32_xor() { raw(0x73); }
	void i32_shl() { raw(0x74); } void i32_shr_s() { raw(0x75); } void i32_shr_u() { raw(0x76); }
	void i32_rotl() { raw(0x77); }
	// wasm MVP に i32/i64 の min・max は無い（0x6D/0x6E は除算、0x52/0x54 は比較）。
	// 一時局所と select で「トップと即値の max/min」を作る
	void i32_max_c(s32 c) { tee(m_scr32); i32_const(c); get(m_scr32); i32_const(c); i32_gt_s(); select(); }
	void i32_min_c(s32 c) { tee(m_scr32); i32_const(c); get(m_scr32); i32_const(c); i32_lt_s(); select(); }
	void i32_clz() { raw(0x67); }
	void i32_eqz() { raw(0x45); }
	void i32_eq() { raw(0x46); } void i32_ne() { raw(0x47); }
	void i32_lt_s() { raw(0x48); } void i32_lt_u() { raw(0x49); }
	void i32_gt_s() { raw(0x4A); } void i32_gt_u() { raw(0x4B); }
	void i32_ge_u() { raw(0x4F); }

	// i64 演算
	void i64_add() { raw(0x7C); } void i64_sub() { raw(0x7D); } void i64_mul() { raw(0x7E); }
	void i64_and() { raw(0x83); } void i64_or() { raw(0x84); } void i64_xor() { raw(0x85); }
	void i64_shl() { raw(0x86); } void i64_shr_s() { raw(0x87); }
	void i64_eqz() { raw(0x50); }
	void i64_eq() { raw(0x51); }
	void i64_lt_s() { raw(0x53); }
	void i64_gt_s() { raw(0x55); }
	// i64 も同様の select クランプ（上の i32_max_c を見よ）
	void i64_max_c(s64 c) { tee(m_scr64); i64_const(c); get(m_scr64); i64_const(c); i64_gt_s(); select(); }
	void i64_min_c(s64 c) { tee(m_scr64); i64_const(c); get(m_scr64); i64_const(c); i64_lt_s(); select(); }

	// 符号変換。wasm に i32.not は無い（0x4D は le_u）。反転は 0-x か xor -1 で作る
	void i32_wrap_i64() { raw(0xA7); }
	void i64_extend_i32_s() { raw(0xAC); }
	void i64_extend_i32_u() { raw(0xAD); }

	// 制御
	void block_void() { raw(0x02); raw(0x40); }        // ネスト番号はこの直後の if/end で数える
	void br_if(u32 depth) { raw(0x0D); uleb(depth); }
	void br(u32 depth) { raw(0x0C); uleb(depth); }
	void if_void() { raw(0x04); raw(0x40); }           // 直前に i32 条件
	void if_i32() { raw(0x04); raw(i32_t); }           // 両腕が i32 を 1 個残す if
	void else_op() { raw(0x05); }
	void end() { raw(0x0B); }
	void drop() { raw(0x1A); }
	// [v1 v2 cond] → cond が 0 でなければ v1
	void select() { raw(0x1B); }

	// env.lfo インポート（func 0）を呼ぶ。引き数は先に積んでおく
	void call_lfo() { raw(0x10); uleb(0); }

	// ---- モジュール全体 ----
	void finish(std::vector<u8> &out) const
	{
		out.clear();
		out.insert(out.end(), { 0x00, 'a', 's', 'm' });
		out.insert(out.end(), { 0x01, 0x00, 0x00, 0x00 });

		// セクション 1: type 2 個
		{
			std::vector<u8> s;
			put_uleb(s, 2);
			s.push_back(0x60); put_uleb(s, 3); s.push_back(i32_t); s.push_back(i32_t); s.push_back(i32_t); put_uleb(s, 0);
			s.push_back(0x60); put_uleb(s, 2); s.push_back(i32_t); s.push_back(i32_t); put_uleb(s, 1); s.push_back(i32_t);
			section(out, 1, s);
		}
		// セクション 2: import 2 個。env.lfo -> type 1（func 0 になる）と
		// env.memory（glue が渡す wasmMemory そのもの。負荷格納がヒープを突く）
		{
			std::vector<u8> s;
			put_uleb(s, 2);
			put_uleb(s, 3); s.push_back('e'); s.push_back('n'); s.push_back('v');
			put_uleb(s, 3); s.push_back('l'); s.push_back('f'); s.push_back('o');
			s.push_back(0x00); put_uleb(s, 1);
			put_uleb(s, 3); s.push_back('e'); s.push_back('n'); s.push_back('v');
			put_uleb(s, 6);
			s.push_back('m'); s.push_back('e'); s.push_back('m');
			s.push_back('o'); s.push_back('r'); s.push_back('y');
			s.push_back(0x02);                           // kind: memory
			s.push_back(0x00);                           // limits: 上限なし
			put_uleb(s, 1);                              // 1 ページ以上あれば足りる
			section(out, 2, s);
		}
		// セクション 3: 関数 1 個（type 0。func 1）
		{
			std::vector<u8> s;
			put_uleb(s, 1); put_uleb(s, 0);
			section(out, 3, s);
		}
		// セクション 7: export "run" -> func 1
		{
			std::vector<u8> s;
			put_uleb(s, 1);
			put_uleb(s, 3); s.push_back('r'); s.push_back('u'); s.push_back('n');
			s.push_back(0x00); put_uleb(s, 1);
			section(out, 7, s);
		}
		// セクション 10: code
		{
			std::vector<u8> body, s;
			// 局所の宣言。まとめるのは得なので 1 個ずつ出す（宣言の個数 = 局所の個数）
			put_uleb(body, m_local_types.size());
			for (u8 t : m_local_types) {
				put_uleb(body, 1);
				body.push_back(t);
			}
			body.insert(body.end(), m_code.begin(), m_code.end());
			body.push_back(0x0B);                        // 関数の end
			put_uleb(s, 1);
			put_uleb(s, body.size());
			s.insert(s.end(), body.begin(), body.end());
			section(out, 10, s);
		}
	}

private:
	static void section(std::vector<u8> &out, u8 id, const std::vector<u8> &s)
	{
		out.push_back(id);
		put_uleb(out, s.size());
		out.insert(out.end(), s.begin(), s.end());
	}

	std::vector<u8> m_code;
	std::vector<u8> m_local_types;                       // 引き数以外の局所の型（番号順）
	u32 m_scr32 = 0, m_scr64 = 0;                        // set_scratch で埋める
};

} // namespace wasmasm

#endif
