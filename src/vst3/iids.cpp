// license:BSD-3-Clause
//
// VST3 のインターフェース番号（iid）の実体をここで一度だけ作る。
//
// ヘッダは番号の「並び」を配るだけで、FUID の実体はどこかの翻訳単位で
// 作らなければならない。Steinberg の SDK では public.sdk 側（GPLv3）に
// 同じものがあるが、中身は下のマクロを並べるだけなので自前で書いた。
// pluginterfaces（MIT）のヘッダだけで完結する。

#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivstunits.h"

namespace Steinberg {

DEF_CLASS_IID (IPlugView)
DEF_CLASS_IID (IPlugFrame)

namespace Vst {

DEF_CLASS_IID (IComponent)
DEF_CLASS_IID (IAudioProcessor)
DEF_CLASS_IID (IAudioPresentationLatency)
DEF_CLASS_IID (IProcessContextRequirements)
DEF_CLASS_IID (IEditController)
DEF_CLASS_IID (IEditController2)
DEF_CLASS_IID (IComponentHandler)
DEF_CLASS_IID (IComponentHandler2)
DEF_CLASS_IID (IMidiMapping)
DEF_CLASS_IID (IUnitInfo)
DEF_CLASS_IID (IEditControllerHostEditing)
DEF_CLASS_IID (IEventList)
DEF_CLASS_IID (IParameterChanges)
DEF_CLASS_IID (IParamValueQueue)
DEF_CLASS_IID (IConnectionPoint)
DEF_CLASS_IID (IMessage)
DEF_CLASS_IID (IAttributeList)

} // namespace Vst
} // namespace Steinberg
