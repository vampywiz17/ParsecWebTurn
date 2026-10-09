#define NOMINMAX
#include <windows.h>
#include <audioclient.h>
#include <codecapi.h>
#include <d3d10_1.h>
#include <d3d11.h>
#include <dxgi.h>
#include <dxgi1_2.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mftransform.h>
#include <mmdeviceapi.h>
#include <wmcodecdsp.h>
#include <wrl/client.h>
#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <iterator>

#pragma comment(lib, "d3d11.lib")
#pragma comment(lib, "dxgi.lib")
#pragma comment(lib, "mf.lib")
#pragma comment(lib, "mfplat.lib")
#pragma comment(lib, "mfuuid.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "wmcodecdspuuid.lib")

using Microsoft::WRL::ComPtr;

extern "C" struct ParsecNativeMediaProbe {
  int32_t video_d3d11_path_ready;
  int32_t video_low_latency_enabled;
  int32_t audio_decoder_ready;
  int32_t wasapi_low_latency_ready;
  uint32_t audio_period_frames;
  uint32_t audio_period_microseconds;
  char adapter[128];
  char video_decoder[128];
  char audio_decoder[128];
  char unavailable_reason[256];
};

namespace {

void CopyText(char *destination, size_t capacity, const char *source) {
  if (capacity == 0) return;
  strncpy_s(destination, capacity, source ? source : "", _TRUNCATE);
}

void CopyWide(char *destination, size_t capacity, const wchar_t *source) {
  if (capacity == 0) return;
  destination[0] = '\0';
  if (!source) return;
  WideCharToMultiByte(CP_UTF8, 0, source, -1, destination,
                      static_cast<int>(capacity), nullptr, nullptr);
  destination[capacity - 1] = '\0';
}

void AddFailure(ParsecNativeMediaProbe *report, const char *stage, HRESULT hr) {
  if (report->unavailable_reason[0] != '\0') return;
  sprintf_s(report->unavailable_reason, "%s failed (HRESULT 0x%08lx)", stage,
            static_cast<unsigned long>(hr));
}

HRESULT ProbeVideo(ParsecNativeMediaProbe *report) {
  constexpr UINT flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT |
                         D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
  constexpr D3D_FEATURE_LEVEL levels[] = {
      D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0,
      D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0};
  ComPtr<ID3D11Device> device;
  ComPtr<ID3D11DeviceContext> context;
  D3D_FEATURE_LEVEL selected{};
  HRESULT hr = D3D11CreateDevice(
      nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, flags, levels,
      static_cast<UINT>(std::size(levels)), D3D11_SDK_VERSION, &device,
      &selected, &context);
  if (hr == E_INVALIDARG) {
    hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, flags,
                           levels + 1,
                           static_cast<UINT>(std::size(levels) - 1),
                           D3D11_SDK_VERSION, &device, &selected, &context);
  }
  if (FAILED(hr)) return hr;

  ComPtr<IDXGIDevice> dxgi_device;
  ComPtr<IDXGIAdapter> adapter;
  ComPtr<IDXGIAdapter1> adapter1;
  DXGI_ADAPTER_DESC1 adapter_desc{};
  if (SUCCEEDED(device.As(&dxgi_device)) &&
      SUCCEEDED(dxgi_device->GetAdapter(&adapter)) &&
      SUCCEEDED(adapter.As(&adapter1)) &&
      SUCCEEDED(adapter1->GetDesc1(&adapter_desc))) {
    if ((adapter_desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) != 0)
      return DXGI_ERROR_UNSUPPORTED;
    CopyWide(report->adapter, std::size(report->adapter),
             adapter_desc.Description);
  }

  ComPtr<ID3D10Multithread> multithread;
  hr = context.As(&multithread);
  if (FAILED(hr)) return hr;
  multithread->SetMultithreadProtected(TRUE);

  UINT reset_token = 0;
  ComPtr<IMFDXGIDeviceManager> manager;
  hr = MFCreateDXGIDeviceManager(&reset_token, &manager);
  if (FAILED(hr)) return hr;
  hr = manager->ResetDevice(device.Get(), reset_token);
  if (FAILED(hr)) return hr;

  ComPtr<IMFTransform> decoder;
  hr = CoCreateInstance(CLSID_CMSH264DecoderMFT, nullptr,
                        CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&decoder));
  if (FAILED(hr)) return hr;
  ComPtr<IMFAttributes> attributes;
  hr = decoder->GetAttributes(&attributes);
  if (FAILED(hr)) return hr;
  UINT32 d3d11_aware = FALSE;
  hr = attributes->GetUINT32(MF_SA_D3D11_AWARE, &d3d11_aware);
  if (FAILED(hr) || !d3d11_aware) return MF_E_UNSUPPORTED_D3D_TYPE;
  hr = decoder->ProcessMessage(
      MFT_MESSAGE_SET_D3D_MANAGER,
      reinterpret_cast<ULONG_PTR>(manager.Get()));
  if (FAILED(hr)) return hr;

  ComPtr<ICodecAPI> codec_api;
  if (SUCCEEDED(decoder.As(&codec_api))) {
    VARIANT enabled;
    VariantInit(&enabled);
    enabled.vt = VT_UI4;
    enabled.ulVal = TRUE;
    if (SUCCEEDED(codec_api->SetValue(
            &CODECAPI_AVDecVideoAcceleration_H264, &enabled)) &&
        SUCCEEDED(codec_api->SetValue(&CODECAPI_AVLowLatencyMode, &enabled))) {
      report->video_low_latency_enabled = 1;
    }
    VariantClear(&enabled);
  }

  ComPtr<IMFMediaType> input;
  hr = MFCreateMediaType(&input);
  if (FAILED(hr)) return hr;
  if (FAILED(hr = input->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video)) ||
      FAILED(hr = input->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_H264_ES)) ||
      FAILED(hr = decoder->SetInputType(0, input.Get(), 0))) {
    return hr;
  }
  report->video_d3d11_path_ready = 1;
  CopyText(report->video_decoder, std::size(report->video_decoder),
           "Microsoft H.264 decoder MFT (D3D11/DXVA path)");
  return S_OK;
}

HRESULT ProbeOpus(ParsecNativeMediaProbe *report) {
  MFT_REGISTER_TYPE_INFO input{MFMediaType_Audio, MFAudioFormat_Opus};
  MFT_REGISTER_TYPE_INFO output{MFMediaType_Audio, MFAudioFormat_Float};
  IMFActivate **activations = nullptr;
  UINT32 count = 0;
  const UINT32 flags = MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT |
                       MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER;
  HRESULT hr = MFTEnumEx(MFT_CATEGORY_AUDIO_DECODER, flags, &input, &output,
                         &activations, &count);
  if (FAILED(hr)) return hr;
  if (count == 0) {
    CoTaskMemFree(activations);
    return MF_E_TOPO_CODEC_NOT_FOUND;
  }
  wchar_t *name = nullptr;
  UINT32 name_length = 0;
  activations[0]->GetAllocatedString(MFT_FRIENDLY_NAME_Attribute, &name,
                                     &name_length);
  if (name) {
    CopyWide(report->audio_decoder, std::size(report->audio_decoder), name);
    CoTaskMemFree(name);
  }
  ComPtr<IMFTransform> decoder;
  hr = activations[0]->ActivateObject(IID_PPV_ARGS(&decoder));
  for (UINT32 i = 0; i < count; ++i) activations[i]->Release();
  CoTaskMemFree(activations);
  if (FAILED(hr)) return hr;

  ComPtr<IMFMediaType> input_type;
  hr = MFCreateMediaType(&input_type);
  if (FAILED(hr)) return hr;
  if (FAILED(hr = input_type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio)) ||
      FAILED(hr = input_type->SetGUID(MF_MT_SUBTYPE, MFAudioFormat_Opus)) ||
      FAILED(hr = input_type->SetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)) ||
      FAILED(hr = input_type->SetUINT32(MF_MT_AUDIO_NUM_CHANNELS, 2)) ||
      FAILED(hr = decoder->SetInputType(0, input_type.Get(), 0))) {
    return hr;
  }
  report->audio_decoder_ready = 1;
  return S_OK;
}

HRESULT ProbeWasapi(ParsecNativeMediaProbe *report) {
  ComPtr<IMMDeviceEnumerator> enumerator;
  HRESULT hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr,
                                CLSCTX_ALL, IID_PPV_ARGS(&enumerator));
  if (FAILED(hr)) return hr;
  ComPtr<IMMDevice> endpoint;
  hr = enumerator->GetDefaultAudioEndpoint(eRender, eConsole, &endpoint);
  if (FAILED(hr)) return hr;
  ComPtr<IAudioClient3> client;
  hr = endpoint->Activate(__uuidof(IAudioClient3), CLSCTX_ALL, nullptr,
                          reinterpret_cast<void **>(client.GetAddressOf()));
  if (FAILED(hr)) return hr;

  WAVEFORMATEX format{};
  format.wFormatTag = WAVE_FORMAT_IEEE_FLOAT;
  format.nChannels = 2;
  format.nSamplesPerSec = 48000;
  format.wBitsPerSample = 32;
  format.nBlockAlign = format.nChannels * format.wBitsPerSample / 8;
  format.nAvgBytesPerSec = format.nSamplesPerSec * format.nBlockAlign;
  WAVEFORMATEX *closest = nullptr;
  hr = client->IsFormatSupported(AUDCLNT_SHAREMODE_SHARED, &format, &closest);
  WAVEFORMATEX *selected = &format;
  if (hr == S_FALSE && closest) {
    selected = closest;
    hr = S_OK;
  }
  if (FAILED(hr)) {
    if (closest) CoTaskMemFree(closest);
    return hr;
  }
  UINT32 default_period = 0, fundamental_period = 0, minimum_period = 0,
         maximum_period = 0;
  hr = client->GetSharedModeEnginePeriod(
      selected, &default_period, &fundamental_period, &minimum_period,
      &maximum_period);
  if (SUCCEEDED(hr)) {
    report->wasapi_low_latency_ready = 1;
    report->audio_period_frames = minimum_period;
    const uint64_t rate = std::max<DWORD>(1, selected->nSamplesPerSec);
    report->audio_period_microseconds = static_cast<uint32_t>(
        (static_cast<uint64_t>(minimum_period) * 1000000ULL) / rate);
  }
  if (closest) CoTaskMemFree(closest);
  return hr;
}

}  // namespace

extern "C" int32_t parsec_native_media_probe(ParsecNativeMediaProbe *report) {
  if (!report) return static_cast<int32_t>(E_POINTER);
  memset(report, 0, sizeof(*report));
  const HRESULT com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
  const bool uninitialize = SUCCEEDED(com);
  if (FAILED(com) && com != RPC_E_CHANGED_MODE) {
    return static_cast<int32_t>(com);
  }
  const HRESULT media = MFStartup(MF_VERSION, MFSTARTUP_LITE);
  if (FAILED(media)) {
    if (uninitialize) CoUninitialize();
    return static_cast<int32_t>(media);
  }

  const HRESULT video = ProbeVideo(report);
  if (FAILED(video)) AddFailure(report, "D3D11 H.264 setup", video);
  const HRESULT opus = ProbeOpus(report);
  if (FAILED(opus)) AddFailure(report, "Opus decoder setup", opus);
  const HRESULT wasapi = ProbeWasapi(report);
  if (FAILED(wasapi)) AddFailure(report, "IAudioClient3 setup", wasapi);

  MFShutdown();
  if (uninitialize) CoUninitialize();
  return 0;
}
