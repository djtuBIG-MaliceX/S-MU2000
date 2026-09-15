#include "SMU2000_VST2.h"

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
  m_engine->start();

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
  m_engine->fill(outputs[0], outputs[1], nFrames, nullptr, nullptr);
}

void SMU2000_VST2::ProcessMidiMsg(const IMidiMsg& msg)
{
  uint8_t bytes[3];
  bytes[0] = msg.mStatus;
  const int nibble = msg.mStatus & 0xF0;
  const int n = (nibble == 0xC0 || nibble == 0xD0) ? 2 : 3;  // ProgramChange/ChannelAT are 2 bytes
  bytes[1] = msg.mData1;
  if (n == 3)
    bytes[2] = msg.mData2;
  m_engine->midi(bytes, n, 0);  // port 0 = parts 1-16; channel lives in the status byte
}

void SMU2000_VST2::ProcessSysEx(const ISysEx& msg)
{
  m_engine->midi(reinterpret_cast<const uint8_t*>(msg.mData), msg.mSize, 0);
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



