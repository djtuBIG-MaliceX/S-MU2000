#pragma once

#include "IPlug_include_in_plug_hdr.h"

#include <memory>
#include <vector>

#include "vst3/engine.h"

#if PLUG_HAS_UI  // GUI-ON only (SMU2000_ENABLE_GUI). Native GDI editor, NO IGraphics.
#include "ui/SMU2000Editor.h"
#endif

const int kNumPresets = 1;
const int kNumParams = 0;  // MIDI-driven instrument, no host params (ledger P2)

using namespace iplug;

class SMU2000_VST2 final : public Plugin
{
public:
  SMU2000_VST2(const InstanceInfo& info);

  void ProcessBlock(sample** inputs, sample** outputs, int nFrames) override;
  void ProcessMidiMsg(const IMidiMsg& msg) override;
  void ProcessSysEx(const ISysEx& msg) override;
  void OnReset() override;

  bool SerializeState(IByteChunk& chunk) const override;
  int UnserializeState(const IByteChunk& chunk, int startPos) override;

#if PLUG_HAS_UI  // native (non-IGraphics) editor: hosts drive these via effEditOpen/Close (VST2)
                 // and guiSetParent/guiDestroy (CLAP). See ui/SMU2000Editor.h.
  void* OpenWindow(void* pParent) override { return m_editor ? m_editor->open(pParent) : nullptr; }
  void  CloseWindow() override { if (m_editor) m_editor->close(); }
  void  OnParentWindowResize(int width, int height) override
  {
    if (m_editor) m_editor->set_size(width, height);
  }
#endif

private:
  std::unique_ptr<smu2000::vst3::engine> m_engine;
#if PLUG_HAS_UI
  std::unique_ptr<smu2000::editor> m_editor;
#endif
};
