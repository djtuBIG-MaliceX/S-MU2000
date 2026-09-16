// license:BSD-3-Clause
//
// The AU's editor: the little that plugin.cpp and editor_mac.mm have to agree
// on. plugin.cpp is plain C++ and answers the properties; editor_mac.mm holds
// the Objective-C class a host instantiates from one of them.
//
// An AUv2 host asks for a custom view with kAudioUnitProperty_CocoaUI, which
// answers with the URL of a bundle and the name of an Objective-C class inside
// it implementing AUCocoaUIBase. That class is editor_mac.mm's, and what it
// builds is the panel the VST3 build already shows -- no second copy of it.
//
// The one awkward part is how the view factory reaches the engine it has to
// draw. It is handed the AudioUnit, and that turns out to be AudioToolbox's own
// handle and not the plug-in's instance (measured: the two pointers differ), so
// it cannot cast it back. It asks for the engine through kEngineProperty
// instead, which travels the ordinary property dispatch and so arrives with the
// instance already resolved.

#ifndef S_MU2000_AU_EDITOR_H
#define S_MU2000_AU_EDITOR_H

#pragma once

#include <AudioToolbox/AudioToolbox.h>

namespace smu2000 {
namespace au {

// The Objective-C class in this bundle that implements AUCocoaUIBase. The
// string is what a host resolves with NSClassFromString, so it has to match the
// @interface in editor_mac.mm exactly -- aubprobe checks both
extern const char *const kViewClassName;

// The engine an open instance is running, as a pointer. 64000 and above is the
// range Apple leaves to everyone else
constexpr AudioUnitPropertyID kEngineProperty = 64000;

// The bundle URL and the view class name for kAudioUnitProperty_CocoaUI, each
// with one reference for the host. False if this bundle cannot be found, in
// which case there is no view to publish and no URL to point at
bool view_info(CFURLRef *out_bundle_url, CFStringRef *out_class_name);

} // namespace au
} // namespace smu2000

#endif // S_MU2000_AU_EDITOR_H
