#pragma once

#include "IPlug_include_in_plug_hdr.h"

#include <cstdint>
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
  // Incoming MIDI is stamped with a sample offset (IMidiMsg::mOffset / ISysEx::mOffset),
  // but smu2000::engine::midi() has no time argument — it just queues bytes onto the
  // emulated 31250 bps serial line. So events are parked here and interleaved into
  // ProcessBlock() at their offset; injecting everything at block start is what made note
  // timing jitter by up to one block (worse at large buffer sizes). Audio-thread only:
  // iPlug2 calls ProcessMidiMsg() just before ProcessBlock() on the same thread.
  struct midi_event
  {
    int offset;                    // sample offset within the coming ProcessBlock()
    int port;                      // engine port (0 = parts 1-16; channel is in the status byte)
    std::vector<uint8_t> bytes;
  };

  std::unique_ptr<smu2000::vst3::engine> m_engine;
  std::vector<midi_event> m_midi_q;
#if PLUG_HAS_UI
  std::unique_ptr<smu2000::editor> m_editor;
#endif
};
