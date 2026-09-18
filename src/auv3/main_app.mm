// license:BSD-3-Clause
//
// AUv3 を入れておくための器のアプリ。
//
// macOS はアプリの中に入っている .appex しか AUv3 として認めないので、
// プラグインだけを配ることができない。このアプリ自体は音を出さない。
// 一度起動するとシステムが中の .appex を見つけ、DAW の一覧に出るようになる。
//
// 窓には ROM がバンドルの中にいるかどうかだけ出す。
// 拡張は砂場の中から自分のバンドルしか読めないので、ROM は作るときに
// make auv3 AUV3_ROMS=roms で入れておく（後から足すと署名が破れる）。

#import <Cocoa/Cocoa.h>

@interface AppDelegate : NSObject <NSApplicationDelegate>
@end

@implementation AppDelegate {
	NSWindow *_window;
}

- (void)applicationDidFinishLaunching:(NSNotification *)note
{
	(void)note;

	NSBundle *appex = nil;
	NSString *plugIns = [[NSBundle mainBundle] builtInPlugInsPath];
	for (NSString *name in [[NSFileManager defaultManager] contentsOfDirectoryAtPath:plugIns
	                                                                           error:nil]) {
		if ([name.pathExtension isEqualToString:@"appex"]) {
			appex = [NSBundle bundleWithPath:[plugIns stringByAppendingPathComponent:name]];
			break;
		}
	}
	NSString *roms = appex ? [appex pathForResource:@"mu2000_flash.bin"
	                                         ofType:nil
	                                    inDirectory:@"roms"] : nil;
	NSString *body;
	if (roms) {
		body = [NSString stringWithFormat:
		        @"ROM: バンドルの中（%@）\n\n"
		         "AudioUnit として登録されています。\n"
		         "DAW の音源の一覧に「S-MU2000 AUv3」が出ます。\n\n"
		         "  出力      MAIN OUT L/R\n"
		         "  入力      A/D INPUT（AD1 が左、AD2 が右）\n"
		         "  MIDI 入   ケーブル 0 = IN A（パート 1-16）\n"
		         "            ケーブル 1 = IN B（パート 17-32）\n"
		         "            ケーブル 2 = IN C（パート 33-48）\n"
		         "            ケーブル 3 = IN D（パート 49-64）\n"
		         "  MIDI 出   MIDI OUT（firmware の返事）\n\n"
		         "AUv2（S-MU2000）とは別物として並びます。",
		        roms.stringByDeletingLastPathComponent];
	} else {
		body = @"ROM がバンドルの中にいません。\n\n"
		        "作るときに make auv3 AUV3_ROMS=roms で入れます。\n"
		        "ROM が無くても登録と描き出しは通りますが、音は出ません。\n"
		        "（記録に「ROM が見つからない」と探した場所が残ります）";
	}

	const NSRect frame = NSMakeRect(0, 0, 560, 340);
	_window = [[NSWindow alloc]
	    initWithContentRect:frame
	              styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
	                         NSWindowStyleMaskMiniaturizable)
	                backing:NSBackingStoreBuffered
	                  defer:NO];
	_window.title = @"S-MU2000 AUv3";
	[_window center];

	NSTextView *text = [[NSTextView alloc] initWithFrame:NSInsetRect(frame, 16, 16)];
	text.editable = NO;
	text.drawsBackground = NO;
	text.font = [NSFont monospacedSystemFontOfSize:12 weight:NSFontWeightRegular];
	text.string = body;

	NSScrollView *scroll = [[NSScrollView alloc] initWithFrame:NSInsetRect(frame, 16, 16)];
	scroll.documentView = text;
	scroll.hasVerticalScroller = YES;
	scroll.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
	_window.contentView = scroll;

	[_window makeKeyAndOrderFront:nil];
	[NSApp activateIgnoringOtherApps:YES];
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)app
{
	(void)app;
	return YES;
}

@end

int main(int argc, const char *argv[])
{
	(void)argc; (void)argv;
	@autoreleasepool {
		NSApplication *app = [NSApplication sharedApplication];
		AppDelegate *del = [[AppDelegate alloc] init];
		app.delegate = del;
		[app setActivationPolicy:NSApplicationActivationPolicyRegular];
		[app run];
	}
	return 0;
}
