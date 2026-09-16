#pragma once

#include "IPlug_include_in_plug_hdr.h"

#include <cstdint>
#include <memory>
#include <type_traits>
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
  //
  // POD event + byte arena (perf): an earlier version embedded a std::vector per event and
  // ran std::stable_sort every block, which malloc'd per MIDI message and per block. That
  // starved dense MIDI files. The event is now a 16-byte trivially-copyable POD referencing
  // m_midi_bytes (append-only byte arena, cleared with the queue every block), so push_back
  // and the sort fallback are raw memmoves and the steady state is allocation-free: the ctor
  // reserves the storage once. iPlug2 drains every offset into the block about to be
  // rendered (offsets are block-relative, queue empty after each ProcessBlock), so no
  // ring/spill logic is needed — clear() + persistent capacity already is the queue.
  // Ordering: ProcessMidiMsg only appends; ProcessBlock scans for sortedness (usually true)
  // and only std::sort's on host disorder. The arena pos doubles as the arrival-order
  // tie-break (bytes are appended in arrival order, so pos is monotonic with it), which
  // reproduces the old stable_sort semantics: equal offsets keep host delivery order.
  struct midi_event  // 16 bytes, trivially copyable
  {
    int      offset;  // sample offset within the coming ProcessBlock()
    uint32_t pos;     // start of this event's bytes in m_midi_bytes
    uint32_t len;
    uint8_t  port;    // engine port (0 = parts 1-16; channel is in the status byte)
  };
  static_assert(std::is_trivially_copyable<midi_event>::value, "memmove-friendly queue");

  void Push(int offset, int port, const uint8_t* bytes, uint32_t n);  // appends event+arena bytes

  std::unique_ptr<smu2000::vst3::engine> m_engine;
  std::vector<midi_event> m_midi_q;
  std::vector<uint8_t>    m_midi_bytes;  // arena backing m_midi_q[].pos; cleared every block
#if PLUG_HAS_UI
  std::unique_ptr<smu2000::editor> m_editor;
#endif
};
