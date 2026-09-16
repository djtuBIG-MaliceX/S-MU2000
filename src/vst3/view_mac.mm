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

#include <algorithm>
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

// The panel's view. Flipped, so the CGContext AppKit hands to drawRect already
// has its origin top-left with y running down -- the space compat/gdi.h assumes
// and the space the panel's hit testing is written in.
@interface SMUPlugView : NSView
{
@public
	plug_view *_owner;
@private
	NSTimer *_timer;
}
- (instancetype)initWithOwner:(plug_view *)owner width:(int)w height:(int)h;
- (void)tick:(NSTimer *)timer;
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

- (void)drawRect:(NSRect)dirty
{
	(void)dirty;
	if (!_owner)
		return;
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
	[self setNeedsDisplay:YES];
}

- (void)viewDidMoveToWindow
{
	[super viewDidMoveToWindow];
	if ([self window]) {
		if (!_timer) {
			_timer = [NSTimer timerWithTimeInterval:1.0 / 30.0
			                                 target:self
			                               selector:@selector(tick:)
			                               userInfo:nil
			                                repeats:YES];
			[[NSRunLoop currentRunLoop] addTimer:_timer forMode:NSRunLoopCommonModes];
		}
	} else {
		[_timer invalidate];
		_timer = nil;
	}
}

// ---- mouse

- (void)mouseDown:(NSEvent *)event
{
	if (!_owner)
		return;
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
}
- (void)choose:(id)sender;
@end

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
}

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
	void alert(const std::string &text) override;

private:
	plug_view &m_owner;
	SMUPlugView *m_view = nil;
};

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

	// In the view's own coordinates. The view is flipped, which is the space the
	// panel's hit testing already worked in
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
	[[m_view window] makeFirstResponder:m_view];

	// Losing key focus must not leave a panel button held down
	[[NSNotificationCenter defaultCenter] addObserver:m_view
	                                         selector:@selector(resignKeyWindow:)
	                                             name:NSWindowDidResignKeyNotification
	                                           object:[m_view window]];
	return true;
}

void mac_window::detach()
{
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


plug_window *plug_window_create(plug_view &owner)
{
	return new mac_window(owner);
}

} // namespace vst3
} // namespace smu2000
