// license:BSD-3-Clause
//
// Windows virtual-key codes translated to the characters the shared keymap
// (ui/keymap.h) wants. Mirrors ui/menu_win.h: menus render there, keys here.
//
// Windows-only: includes <windows.h> for the VK_ codes. Header-only (inline)
// so no build system changes are needed.

#ifndef S_MU2000_UI_KEYMAP_WIN_H
#define S_MU2000_UI_KEYMAP_WIN_H

#pragma once

#include <windows.h>

#include "menu.h"      // KEY_F2..F5: the shared key space above ASCII

namespace ui {

// A virtual-key code translated to the shared key space (keymap.h): the
// panel characters, the four F-keys the app acts on, or 0 for no panel key.
// Letters lowercased, punctuation from its OEM code.
inline int key_char_of_vk(int vk)
{
	if (vk >= 'A' && vk <= 'Z')
		return char(vk - 'A' + 'a');
	switch (vk) {
	case VK_F2: return KEY_F2;
	case VK_F3: return KEY_F3;
	case VK_F4: return KEY_F4;
	case VK_F5: return KEY_F5;
	case VK_OEM_6: return ']';
	case VK_OEM_4: return '[';
	case VK_OEM_PLUS: return '=';
	case VK_OEM_MINUS: return '-';
	case VK_OEM_PERIOD: return '.';
	case VK_OEM_COMMA: return ',';
	case VK_BACK: return '\b';
	case VK_RETURN: return '\r';
	default: break;
	}
	return 0;
}

} // namespace ui

#endif // S_MU2000_UI_KEYMAP_WIN_H
