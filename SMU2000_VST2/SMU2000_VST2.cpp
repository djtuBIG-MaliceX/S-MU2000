#include "SMU2000_VST2.h"

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <utility>

#if defined(CLAP_API) && (defined(__GNUC__) || defined(__clang__))
// MinGW/Clang portability fix (no iPlug2 submodule edit; mirrors ../sw10_plug). GCC/Clang
// reject __attribute__((dllexport)) on a namespace-scope `const` definition even with a
// prior extern declaration (unlike MSVC, which tolerates it). Upstream defines the CLAP
// entry points as `CLAP_EXPORT const clap_plugin_*_t x = {...}` in
// IPlug_include_in_plug_src.h (next include). Locally expand CLAP_EXPORT to
// `extern __attribute__((dllexport))` (external linkage + export) so those definitions
// are valid and exported (clap_entry). All of the CLAP SDK's own CLAP_EXPORT uses were
// already processed through the header chain above (header-guarded), so this stays
// local to the entry points. MSVC path unaffected (guard excludes it); VST2 path
// unaffected (CLAP_API undefined).
#undef  CLAP_EXPORT
#define CLAP_EXPORT extern __attribute__((dllexport))
#endif

#include "IPlug_include_in_plug_src.h"

#if defined(CLAP_API) && (defined(__GNUC__) || defined(__clang__))
// Restore the SDK spelling for anything compiled after this point in this TU.
#undef CLAP_EXPORT
#if defined(_WIN32) || defined(__CYGWIN__)
#define CLAP_EXPORT __attribute__((dllexport))
#else
#define CLAP_EXPORT __attribute__((visibility("default")))
#endif
#endif

static_assert(sizeof(sample) == 4, "target must define SAMPLE_TYPE_FLOAT (engine::fill is float)");

// P7 guard: SMU2000_ENABLE_GUI (GUI-ON build) must survive into PLUG_HAS_UI, or the host
// silently gets no editor (HasUI()==false -> effEditGetRect/effEditOpen are no-ops). If
// this trips, config.h's PLUG_HAS_UI guard is desynced from the per-target define.
#if defined(SMU2000_ENABLE_GUI)
static_assert(PLUG_HAS_UI == 1, "SMU2000_ENABLE_GUI set but PLUG_HAS_UI != 1 (config desync)");
#endif

SMU2000_VST2::SMU2000_VST2(const InstanceInfo& info)
  // Qualified on purpose: CLAP's base clap::helpers::Plugin injects the name
  // "Plugin" into class scope (C2614 for the unqualified alias). Same spelling
  // as upstream Examples/IPlugEffect.cpp; correct for both VST2_API and CLAP_API.
  : iplug::Plugin(info, MakeConfig(kNumParams, kNumPresets))
{
  m_engine = std::make_unique<smu2000::vst3::engine>();
  m_engine->set_output_rate(GetSampleRate());
  // Boot synchronously here: the host starts its timeline the moment the instance
  // exists and a plug-in cannot pause it, so the old async boot streamed the first
  // 2-5 s of every song out as silence with the song-start MIDI queued behind it
  // (render.exe avoids this by running the boot loop before feeding MIDI from
  // position 0). The constructor runs before audio streaming, so this is where
  // "delay the streaming start until live" has to happen — state() is ready (or
  // failed) when the ctor returns. The midi() queue + fill() silence stay as the
  // safety net (and for the async opt-out: plugin.ini boot=async / SMU2000_SYNC_BOOT=0).
  // A post-boot snapshot (bootcache.bin) makes only the first cold instance pay.
  m_engine->start(true);

#if PLUG_HAS_UI  // native GDI editor (SMU2000_ENABLE_GUI=ON); untouched when OFF. Attaches to
                 // the host HWND on OpenWindow (effEditOpen / guiSetParent). No IGraphics.
  m_editor = std::make_unique<smu2000::editor>(*m_engine);
  SetEditorSize(m_editor->width(), m_editor->height());
#endif
}

void SMU2000_VST2::OnReset()
{
  m_engine->set_output_rate(GetSampleRate());
  m_engine->set_processing(true);
}

void SMU2000_VST2::ProcessBlock(sample** inputs, sample** outputs, int nFrames)
{
  (void) inputs;  // PLUG_CHANNEL_IO "0-2": no audio inputs

  // No MIDI this block: render it in one call (the common, lowest-overhead case).
  if (m_midi_q.empty())
  {
    m_engine->fill(outputs[0], outputs[1], nFrames, nullptr, nullptr);
    return;
  }

  // Hosts deliver events unordered within effProcessEvents / in_events; the engine
  // consumes its serial queue sample-sequentially, so order by sample offset first.
  // stable_sort keeps host ordering for events sharing an offset.
  std::stable_sort(m_midi_q.begin(), m_midi_q.end(),
                   [](const midi_event& a, const midi_event& b) { return a.offset < b.offset; });

  // Walk the block in segments: render up to each event's offset, then clock that
  // event's bytes onto the serial line, so note on/off lands at the right sample
  // instead of the block start. Each fill() releases m_machine on return, so calling
  // midi() between fill() calls is lock-safe; the 31250 bps model then spaces bytes.
  int produced = 0;
  size_t i = 0;
  while (i < m_midi_q.size())
  {
    int off = m_midi_q[i].offset;
    if (off < 0) off = 0;
    if (off > nFrames) off = nFrames;  // clamp past-the-end to the block tail

    if (off > produced)
    {
      m_engine->fill(outputs[0] + produced, outputs[1] + produced, off - produced, nullptr, nullptr);
      produced = off;
    }

    // All events landing on this sample go onto the line before the next run_sample().
    while (i < m_midi_q.size() && m_midi_q[i].offset <= off)
    {
      m_engine->midi(m_midi_q[i].bytes.data(), m_midi_q[i].bytes.size(), m_midi_q[i].port);
      ++i;
    }
  }

  if (produced < nFrames)
    m_engine->fill(outputs[0] + produced, outputs[1] + produced, nFrames - produced, nullptr, nullptr);

  m_midi_q.clear();
}

void SMU2000_VST2::ProcessMidiMsg(const IMidiMsg& msg)
{
  midi_event ev;
  ev.offset = msg.mOffset;  // sample offset into the coming ProcessBlock()
  ev.port = 0;              // port 0 = parts 1-16; channel lives in the status byte
  const int nibble = msg.mStatus & 0xF0;
  const int n = (nibble == 0xC0 || nibble == 0xD0) ? 2 : 3;  // ProgramChange/ChannelAT are 2 bytes
  ev.bytes.reserve(n);
  ev.bytes.push_back(msg.mStatus);
  ev.bytes.push_back(msg.mData1);
  if (n == 3)
    ev.bytes.push_back(msg.mData2);
  m_midi_q.push_back(std::move(ev));
}

void SMU2000_VST2::ProcessSysEx(const ISysEx& msg)
{
  midi_event ev;
  ev.offset = msg.mOffset;
  ev.port = 0;
  const uint8_t* p = reinterpret_cast<const uint8_t*>(msg.mData);
  ev.bytes.assign(p, p + msg.mSize);
  m_midi_q.push_back(std::move(ev));
}

bool SMU2000_VST2::SerializeState(IByteChunk& chunk) const
{
  const std::vector<uint8_t> blob = m_engine->save_state();
  chunk.PutBytes(blob.data(), (int) blob.size());
  return true;
}

int SMU2000_VST2::UnserializeState(const IByteChunk& chunk, int startPos)
{
  const int n = chunk.Size() - startPos;
  if (n > 0)
  {
    std::vector<uint8_t> blob(n);
    chunk.GetBytes(blob.data(), n, startPos);
    m_engine->load_state(blob.data(), (size_t) n);
  }
  return chunk.Size();
}



