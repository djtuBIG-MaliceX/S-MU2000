// license:BSD-3-Clause
//
// The VST3 view's window on macOS: an NSView added to whatever view the host
// hands over in IPlugView::attached(). The VST3 interface, the panel and the
// input semantics all live in view.cpp; this is only the window.
//
// Objective-C++ on purpose, and the only VST3 file that is. Cocoa's headers
// define BOOL and Quickdraw's define Polygon, while compat/gdi.h has to declare
// both so panel.cpp can stay untouched. view.h mentions no window system at
// all, so this file includes that and never includes gdi.h -- the panel is
// reached through the void* entry points on plug_view.

#include "plug_window.h"
#include "view.h"

#import <Cocoa/Cocoa.h>

#include "ui/fx_editor.h"
#include "ui/master_editor.h"
#include "ui/overview.h"
#include "ui/part_shapes.h"
#include "ui/pc_editor.h"
#include "ui/pc_host.h"
#include "ui/pc_window_mac.h"
#include "ui/xg_ui.h"

#include <algorithm>
#include <cstdio>
#include <memory>
#include <string>

using namespace Steinberg;

namespace smu2000 {
namespace vst3 {

const char *plug_window_type() { return kPlatformTypeNSView; }

namespace {

// plug_key_of()'s counterpart for this platform. A character where the key has
// one, which is what the GUI front end maps too, so the same physical key is
// the same panel button in both programs.
int plug_key_of_char(int c)
{
	switch (c) {
	case 'a': return PLUG_KEY_PLAY;
	case 'e': return PLUG_KEY_EDIT;
	case 'u': return PLUG_KEY_UTIL;
	case 'f': return PLUG_KEY_EFFECT;
	case 's': return PLUG_KEY_MUTE_SOLO;
	case ']': return PLUG_KEY_PART_PLUS;
	case '[': return PLUG_KEY_PART_MINUS;
	case '=': case '+': return PLUG_KEY_VALUE_PLUS;
	case '-': return PLUG_KEY_VALUE_MINUS;
	case '\r': return PLUG_KEY_ENTER;
	case 0x7f: case 0x08: return PLUG_KEY_EXIT;
	case '.': return PLUG_KEY_SELECT_RIGHT;
	case ',': return PLUG_KEY_SELECT_LEFT;
	case 'q': return PLUG_KEY_SEQ;
	case 'z': return PLUG_KEY_AUDITION;
	case 'x': return PLUG_KEY_SELECT;
	case 'm': return PLUG_KEY_SAMPLING_MODE;
	// The same two keys gui.exe uses, as their private-use characters
	// (NSF3FunctionKey / NSF2FunctionKey)
	case 0xf706: return PLUG_KEY_LIST;
	case 0xf705: return PLUG_KEY_EDITOR;
	case 0xf707: return PLUG_KEY_ENGINE;   // NSF4FunctionKey
	default: break;
	}
	return PLUG_KEY_NONE;
}

} // namespace
} // namespace vst3
} // namespace smu2000

// The panel's view is declared at global scope on purpose: clang accepts an
// Objective-C class declared inside a namespace, but its ivars stop resolving
// there, and every method of this one touches them. The two names the methods
// need are pulled in by hand, since unqualified lookup from here cannot see
// into the namespaces above
using smu2000::vst3::plug_view;
using smu2000::vst3::plug_key_of_char;
using smu2000::vst3::PLUG_KEY_NONE;

// mac_window, defined below: the card menu's choices open its PC windows
namespace smu2000 { namespace vst3 { class mac_window; } }

// The panel's view. Flipped, so the CGContext AppKit hands to drawRect already
// has its origin top-left with y running down -- the space compat/gdi.h assumes
// and the space the panel's hit testing is written in.
@interface SMUPlugView : NSView
{
@public
	plug_view *_owner;
@private
	NSTimer *_timer;
	int _clicks;
	int _moves;
	int _ticks;
}
- (instancetype)initWithOwner:(plug_view *)owner width:(int)w height:(int)h;
- (void)tick:(NSTimer *)timer;
- (void)ensureTimer;
- (int)plugKeyForEvent:(NSEvent *)event;
@end

@implementation SMUPlugView

- (instancetype)initWithOwner:(plug_view *)owner width:(int)w height:(int)h
{
	self = [super initWithFrame:NSMakeRect(0, 0, w, h)];
	if (self)
		_owner = owner;
	return self;
}

- (void)dealloc
{
	[_timer invalidate];
}

- (BOOL)isFlipped { return YES; }
- (BOOL)acceptsFirstResponder { return YES; }

// A host that keeps a plug-in's editor in a panel which does not take key focus
// (Waveform does) makes every click into it a "first mouse" click, and AppKit
// hands that click to the window to activate rather than to the view -- so the
// panel draws and animates but no button ever fires. Saying yes here is what
// lets the click through as well as activating the window
- (BOOL)acceptsFirstMouse:(NSEvent *)event
{
	(void)event;
	return YES;
}

- (void)drawRect:(NSRect)dirty
{
	(void)dirty;
	if (!_owner)
		return;
	// The timer normally starts in viewDidMoveToWindow, but some hosts move
	// the view around in ways that leave it windowless there and never move
	// it again: with no timer the panel paints once and freezes. Drawing
	// always runs on the main thread with a window in place, so a missing
	// timer is remade here instead of staying missing
	if (!_timer && [self window])
		[self ensureTimer];
	CGContextRef ctx = [[NSGraphicsContext currentContext] CGContext];
	if (!ctx)
		return;
	const NSRect b = [self bounds];
	_owner->repaint((void *)ctx, (int)b.size.width, (int)b.size.height);
}

// Repaint at the same 30 frames a second the Win32 window uses, so the two
// panels animate alike. Common modes so it keeps ticking during a live resize
- (void)tick:(NSTimer *)timer
{
	(void)timer;
	if (_ticks < 3) {
		_ticks++;
		if (_owner && _ticks == 1)
			_owner->log_line("panel timer: first tick");
	}
	[self setNeedsDisplay:YES];
}

// Start the repaint timer unless one already runs. Safe to call twice
- (void)ensureTimer
{
	if (_timer)
		return;
	_timer = [NSTimer timerWithTimeInterval:1.0 / 30.0
	                                 target:self
	                               selector:@selector(tick:)
	                               userInfo:nil
	                                repeats:YES];
	[[NSRunLoop currentRunLoop] addTimer:_timer forMode:NSRunLoopCommonModes];
}

- (void)viewDidMoveToWindow
{
	[super viewDidMoveToWindow];
	NSWindow *win = [self window];
	// The key notification is observed per window, and the window is only known
	// here: a plug-in view is built before the host has put it anywhere, so
	// asking for [self window] while attaching answers nil -- and nil there means
	// "every window", which is not the question being asked
	[[NSNotificationCenter defaultCenter] removeObserver:self
	                                                name:NSWindowDidResignKeyNotification
	                                              object:nil];
	if (win) {
		if (_moves < 4) {
			_moves++;
			if (_owner) {
				char b[96];
				std::snprintf(b, sizeof(b), "panel timer: moved to window (%s timer)",
				              _timer ? "keeping" : "starting");
				_owner->log_line(b);
			}
		}
		[self ensureTimer];
		[[NSNotificationCenter defaultCenter] addObserver:self
		                                         selector:@selector(resignKeyWindow:)
		                                             name:NSWindowDidResignKeyNotification
		                                           object:win];

		// Keyboard focus is asked for here and not in attached(), because this is
		// the first moment the window is known: a view is built before the host
		// has put it anywhere, so asking [self window] there answers nil and the
		// ask goes nowhere. Only when the window itself owns the focus -- a host
		// that keeps another control in the same window keeps it
		id first = [win firstResponder];
		if (!first || first == win)
			[win makeFirstResponder:self];
	} else {
		if (_moves < 4) {
			_moves++;
			if (_owner)
				_owner->log_line("panel timer: moved out of window (timer stopped)");
		}
		[_timer invalidate];
		_timer = nil;
	}
}

// Window notifications arrive as a message to the observer, and the selector is
// named above. **NSView has no -resignKeyWindow** (NSWindow does), so without
// this the notification would send an unrecognised selector and take the host
// down with it the first time the editor window lost focus
- (void)resignKeyWindow:(NSNotification *)note
{
	(void)note;
	if (_owner)
		_owner->focus_lost();
}

// ---- mouse

// What a host did with a click is not visible from outside the view: the panel
// draws and animates either way, so "it is on screen, updating, and answering
// nothing" can only be told apart by writing down what arrived. The first few
// clicks go to the log with the three answers that name the cases:
//
//   key no       the window never takes key focus, so AppKit spends the click
//                activating it and the view is never asked -- this is the one
//                -acceptsFirstMouse fixes, and the one that leaves no line here
//                at all when the host is stubborn
//   main no      the click came in on a thread that must not touch views
//   hit other    something the host put on top took the click first
- (void)noteClick:(NSEvent *)event
{
	if (!_owner || _clicks >= 8)
		return;
	_clicks++;

	NSWindow *win = [self window];
	NSView *hit = win ? [[win contentView] hitTest:[event locationInWindow]] : nil;
	char b[200];
	std::snprintf(b, sizeof(b), "クリック %d 回目: 窓 %s、key %s、主の糸 %s、当たった先 %s",
	              _clicks, win ? "あり" : "なし",
	              (win && [win isKeyWindow]) ? "yes" : "no",
	              [NSThread isMainThread] ? "yes" : "no",
	              hit ? NSStringFromClass([hit class]).UTF8String : "なし");
	_owner->log_line(b);
}

- (void)mouseDown:(NSEvent *)event
{
	if (!_owner)
		return;
	[self noteClick:event];
	NSPoint p = [self convertPoint:[event locationInWindow] fromView:nil];
	_owner->mouse_down((int)p.x, (int)p.y);
	[self setNeedsDisplay:YES];
}

- (void)mouseDragged:(NSEvent *)event
{
	if (!_owner)
		return;
	NSPoint p = [self convertPoint:[event locationInWindow] fromView:nil];
	_owner->mouse_drag((int)p.x, (int)p.y);
	[self setNeedsDisplay:YES];
}

- (void)mouseUp:(NSEvent *)event
{
	(void)event;
	if (!_owner)
		return;
	_owner->mouse_up();
	[self setNeedsDisplay:YES];
}

- (void)scrollWheel:(NSEvent *)event
{
	if (!_owner)
		return;

	CGFloat dy = [event scrollingDeltaY];
	if ([event isDirectionInvertedFromDevice])
		dy = -dy;

	int steps = [event hasPreciseScrollingDeltas] ? (int)(dy / 10.0) : (int)dy;
	if (!steps)
		return;

	NSPoint p = [self convertPoint:[event locationInWindow] fromView:nil];
	_owner->wheel((int)p.x, (int)p.y, steps);
	[self setNeedsDisplay:YES];
}

// ---- keys

- (int)plugKeyForEvent:(NSEvent *)event
{
	NSString *chars = [[event charactersIgnoringModifiers] lowercaseString];
	if ([chars length] < 1)
		return PLUG_KEY_NONE;
	return plug_key_of_char((int)[chars characterAtIndex:0]);
}

- (void)keyDown:(NSEvent *)event
{
	if (!_owner) {
		[super keyDown:event];
		return;
	}
	// Buttons latch while held, so autorepeat would read as a stream of presses
	if ([event isARepeat])
		return;
	const int k = [self plugKeyForEvent:event];
	if (k != PLUG_KEY_NONE)
		_owner->key(k, true);
}

- (void)keyUp:(NSEvent *)event
{
	if (!_owner)
		return;
	const int k = [self plugKeyForEvent:event];
	if (k != PLUG_KEY_NONE)
		_owner->key(k, false);
}

- (void)rightMouseDown:(NSEvent *)event
{
	if (!_owner)
		return;
	[self noteClick:event];
	NSPoint p = [self convertPoint:[event locationInWindow] fromView:nil];
	_owner->mouse_right((int)p.x, (int)p.y);
}

@end


// The card menu's target. NSMenu sends each choice to one object, and the size
// items are told apart by their tag; this object turns that into a call on the
// view (the same four jobs view.cpp's card_* methods do for either platform)
@interface SMUCardMenu : NSObject
{
@public
	plug_view *_owner;
	smu2000::vst3::mac_window *_win;
}
- (void)choose:(id)sender;
@end


namespace smu2000 {
namespace vst3 {

class mac_window : public plug_window
{
public:
	explicit mac_window(plug_view &owner) : m_owner(owner) {}
	~mac_window() override { detach(); }

	bool attach(void *parent, int w, int h) override;
	void detach() override;
	void set_size(int w, int h) override;
	void card_menu(int x, int y) override;
	void panel_menu(int x, int y) override;
	void alert(const std::string &text) override;
	void pc_frame(::xg::model &m, const ::ui::xg_snapshot &ram, ::ui::bridge &br) override;

	// Open a PC window (overview/editor), showing an alert when it fails
	void open_pc(ui::pc_window &w);
	void open_pc_window(int kind) override
	{
		switch (kind) {
		case PC_EDITOR: open_pc(m_editor); break;
		case PC_FX:     open_pc(m_fx);     break;
		case PC_SHAPES: open_pc(m_shapes); break;
		case PC_MASTER: open_pc(m_master); break;
		default:        open_pc(m_list);   break;
		}
	}

private:
	plug_view &m_owner;
	SMUPlugView *m_view = nil;
	// The PC windows gui.exe shows (overview, editor, insertion, part voice,
	// master). Same content as on Windows; only the hosting window differs
	ui::pc_window m_list{ std::make_unique<ui::overview>() };
	ui::pc_window m_editor{ std::make_unique<ui::pc_editor>() };
	ui::pc_window m_fx{ std::make_unique<ui::fx_editor>() };
	ui::pc_window m_shapes{ std::make_unique<ui::part_shapes>() };
	ui::pc_window m_master{ std::make_unique<ui::master_editor>() };
};

// Objective-C lives at global scope (see the note on SMUPlugView above);
// the mac_window methods resume inside the namespaces below
} // namespace vst3
} // namespace smu2000

@implementation SMUCardMenu

- (void)choose:(id)sender
{
	if (!_owner)
		return;
	const int tag = (int)[sender tag];

	if (tag == 1 || tag == 2 || tag == 4 || tag == 8) {          // 16 / 32 / 64 / 128 MB
		NSSavePanel *panel = [NSSavePanel savePanel];
		[panel setTitle:@"新しい SmartMedia の保存先"];
		[panel setNameFieldStringValue:@"smartmedia.img"];
		[panel setAllowedFileTypes:@[ @"img" ]];
		if ([panel runModal] != NSModalResponseOK)
			return;
		_owner->card_make(std::string([[[panel URL] path] UTF8String]), tag * 16);
		return;
	}

	if (tag == 9) {                                              // 差す
		NSOpenPanel *panel = [NSOpenPanel openPanel];
		[panel setTitle:@"差す SmartMedia"];
		[panel setCanChooseFiles:YES];
		[panel setCanChooseDirectories:NO];
		[panel setAllowsMultipleSelection:NO];
		if ([panel runModal] != NSModalResponseOK)
			return;
		_owner->card_insert_path(std::string([[[panel URL] path] UTF8String]));
		return;
	}

	if (tag == 10)                                               // 抜く
		_owner->card_eject();
	else if (tag == 11 && _win)                                  // 一覧
		_win->open_list();
	else if (tag == 12 && _win)                                  // エディタ
		_win->open_editor();
}

@end

namespace smu2000 {
namespace vst3 {

void mac_window::alert(const std::string &text)
{
	NSAlert *a = [[NSAlert alloc] init];
	[a setMessageText:@"S-MU2000"];
	[a setInformativeText:[NSString stringWithUTF8String:text.c_str()]];
	[a addButtonWithTitle:@"OK"];
	[a runModal];
}

// The card slot's menu, offered as a native popup. A card menu needs a target
// to receive the choice, so one is made per call and released as the menu goes
void mac_window::card_menu(int x, int y)
{
	if (!m_view)
		return;

	SMUCardMenu *target = [[SMUCardMenu alloc] init];
	target->_owner = &m_owner;
	target->_win = this;

	NSMenu *m = [[NSMenu alloc] init];
	[m setAutoenablesItems:NO];

	NSMenuItem *item = [m addItemWithTitle:@"新しい SmartMedia を作って差す" action:nil keyEquivalent:@""];
	NSMenu *sizes = [[NSMenu alloc] init];
	const int mbs[4] = { 16, 32, 64, 128 };
	const int tags[4] = { 1, 2, 4, 8 };
	for (int i = 0; i < 4; i++) {
		NSMenuItem *size = [[NSMenuItem alloc] initWithTitle:[NSString stringWithFormat:@"%dMB", mbs[i]]
		                                              action:@selector(choose:)
		                                       keyEquivalent:@""];
		[size setTarget:target];
		[size setTag:tags[i]];
		[size setEnabled:m_owner.card_ready() ? YES : NO];
		[sizes addItem:size];
	}
	[m setSubmenu:sizes forItem:item];

	item = [m addItemWithTitle:@"SmartMedia を差す..." action:@selector(choose:) keyEquivalent:@""];
	[item setTarget:target];
	[item setTag:9];
	[item setEnabled:m_owner.card_ready() ? YES : NO];

	// The card in the slot, by file name, so it is clear which one is going out
	const std::string path = m_owner.card_path();
	NSString *eject_title = @"SmartMedia を抜く";
	if (!path.empty()) {
		const size_t slash = path.find_last_of("/");
		NSString *name = [NSString stringWithUTF8String:path.substr(slash == std::string::npos ? 0 : slash + 1).c_str()];
		eject_title = [NSString stringWithFormat:@"SmartMedia を抜く（%@）", name];
	}
	item = [m addItemWithTitle:eject_title action:@selector(choose:) keyEquivalent:@""];
	[item setTarget:target];
	[item setTag:10];
	[item setEnabled:path.empty() ? NO : YES];

	// The PC windows, where the Windows menu has them
	[m addItem:[NSMenuItem separatorItem]];
	item = [m addItemWithTitle:@"一覧を開く" action:@selector(choose:) keyEquivalent:@""];
	[item setTarget:target];
	[item setTag:11];
	item = [m addItemWithTitle:@"エディタを開く" action:@selector(choose:) keyEquivalent:@""];
	[item setTarget:target];
	[item setTag:12];

	// In the view's own coordinates. The view is flipped, which is the space the
	// panel's hit testing already worked in
	[m popUpMenuPositioningItem:nil atLocation:NSMakePoint(x, y) inView:m_view];
}

void mac_window::panel_menu(int x, int y)
{
	if (!m_view)
		return;

	SMUCardMenu *target = [[SMUCardMenu alloc] init];
	target->_owner = &m_owner;
	target->_win = this;

	NSMenu *m = [[NSMenu alloc] init];
	[m setAutoenablesItems:NO];

	NSMenuItem *item = [m addItemWithTitle:@"一覧を開く" action:@selector(choose:) keyEquivalent:@""];
	[item setTarget:target];
	[item setTag:11];
	item = [m addItemWithTitle:@"エディタを開く" action:@selector(choose:) keyEquivalent:@""];
	[item setTarget:target];
	[item setTag:12];

	[m popUpMenuPositioningItem:nil atLocation:NSMakePoint(x, y) inView:m_view];
}

bool mac_window::attach(void *parent, int w, int h)
{
	if (m_view || !parent)
		return false;

	// The host hands over its view as a bare pointer; on this platform that is
	// NSView (kPlatformTypeNSView). Never nil in practice, but a bad pointer
	// here would fault rather than fail, so check the obvious
	NSView *host = (__bridge NSView *)parent;
	if (!host || ![host isKindOfClass:[NSView class]])
		return false;

	m_view = [[SMUPlugView alloc] initWithOwner:&m_owner width:w height:h];
	if (!m_view)
		return false;

	// Subview, not the content view: the host owns the window and may put other
	// things around us
	[host addSubview:m_view];
	[m_view setFrame:NSMakeRect(0, 0, w, h)];
	// First responder is asked for from viewDidMoveToWindow: the window is not
	// known here yet (this runs before the host has shown the view), and asking
	// then answers nil. Waiting until the view is somewhere can tell a host's own
	// window from ours, which is what keeps the editor from pulling the keyboard
	// away from whatever the host holds focus with

	// Losing key focus must not leave a panel button held down. The observer is
	// registered by the view itself in viewDidMoveToWindow, which is where the
	// window it belongs to is known
	return true;
}

void mac_window::detach()
{
	// The hosted PC windows go with the panel: left open they would keep
	// drawing from an engine that is being torn down
	m_list.hide();
	m_editor.hide();
	m_fx.hide();
	m_shapes.hide();
	m_master.hide();
	if (m_view) {
		[[NSNotificationCenter defaultCenter] removeObserver:m_view];
		[m_view removeFromSuperview];
		m_view->_owner = nullptr;
		m_view = nil;
	}
}

void mac_window::set_size(int w, int h)
{
	if (m_view)
		[m_view setFrame:NSMakeRect(0, 0, w, h)];
}

void mac_window::open_pc(ui::pc_window &w)
{
	std::string err;
	if (!w.show(err))
		alert(err.empty() ? std::string("the window cannot be opened") : err);
}

// Driven at the panel's repaint rate. Hidden windows cost nothing
void mac_window::pc_frame(::xg::model &m, const ::ui::xg_snapshot &ram, ::ui::bridge &br)
{
	ui::pc_frame_all(m_list, m_editor, m_fx, m_shapes, m_master, m, ram, br,
	                 [this](ui::pc_window &w) { open_pc(w); });
}


plug_window *plug_window_create(plug_view &owner)
{
	return new mac_window(owner);
}

} // namespace vst3
} // namespace smu2000
