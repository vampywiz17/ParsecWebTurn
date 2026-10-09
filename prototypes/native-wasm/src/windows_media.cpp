#define NOMINMAX
#include <windows.h>
#include <audioclient.h>
#include <codecapi.h>
#include <d3d10_1.h>
#include <d3d11.h>
#include <dxgi.h>
#include <dxgi1_2.h>
#include <dxgi1_3.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mftransform.h>
#include <mmdeviceapi.h>
#include <wmcodecdsp.h>
#include <wrl/client.h>
#include <algorithm>
#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <iterator>
#include <mutex>
#include <new>
#include <thread>
#include <vector>

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

extern "C" struct ParsecNativeMediaStats {
  int32_t video_ready;
  int32_t audio_ready;
  uint64_t video_packets_submitted;
  uint64_t video_frames_decoded;
  uint64_t video_frames_presented;
  uint64_t video_frames_dropped;
  uint32_t video_width;
  uint32_t video_height;
  uint64_t audio_packets_submitted;
  uint64_t audio_pcm_frames_decoded;
  uint64_t audio_pcm_frames_rendered;
  uint64_t audio_frames_dropped;
  uint64_t audio_underflows;
  int32_t last_hresult;
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

struct LiveCounters {
  std::atomic<uint64_t> video_packets{0};
  std::atomic<uint64_t> video_decoded{0};
  std::atomic<uint64_t> video_presented{0};
  std::atomic<uint64_t> video_dropped{0};
  std::atomic<uint64_t> audio_packets{0};
  std::atomic<uint64_t> audio_decoded{0};
  std::atomic<uint64_t> audio_rendered{0};
  std::atomic<uint64_t> audio_dropped{0};
  std::atomic<uint64_t> audio_underflows{0};
};

class AudioRenderer {
 public:
  explicit AudioRenderer(LiveCounters *counters) : counters_(counters) {}
  ~AudioRenderer() { Stop(); }

  HRESULT Start() {
    thread_ = std::thread([this] { Run(); });
    std::unique_lock<std::mutex> lock(mutex_);
    ready_cv_.wait(lock, [this] { return initialized_; });
    return initialize_result_;
  }

  void Stop() {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      stop_ = true;
    }
    if (event_) SetEvent(event_);
    if (thread_.joinable()) thread_.join();
    if (event_) CloseHandle(event_);
    event_ = nullptr;
  }

  void Push(const float *samples, size_t frames) {
    std::lock_guard<std::mutex> lock(mutex_);
    const size_t incoming = frames * 2;
    const size_t capacity = static_cast<size_t>(period_frames_) * 2 * 4;
    while (capacity && queue_.size() + incoming > capacity &&
           queue_.size() >= 2) {
      queue_.pop_front();
      queue_.pop_front();
      counters_->audio_dropped.fetch_add(1, std::memory_order_relaxed);
    }
    queue_.insert(queue_.end(), samples, samples + incoming);
  }

 private:
  void FinishInitialization(HRESULT result) {
    {
      std::lock_guard<std::mutex> lock(mutex_);
      initialize_result_ = result;
      initialized_ = true;
    }
    ready_cv_.notify_one();
  }

  void Run() {
    const HRESULT com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    const bool uninitialize = SUCCEEDED(com);
    ComPtr<IMMDeviceEnumerator> enumerator;
    ComPtr<IMMDevice> endpoint;
    ComPtr<IAudioClient3> client;
    ComPtr<IAudioRenderClient> render;
    HRESULT hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr,
                                  CLSCTX_ALL, IID_PPV_ARGS(&enumerator));
    if (SUCCEEDED(hr))
      hr = enumerator->GetDefaultAudioEndpoint(eRender, eConsole, &endpoint);
    if (SUCCEEDED(hr))
      hr = endpoint->Activate(__uuidof(IAudioClient3), CLSCTX_ALL, nullptr,
                              reinterpret_cast<void **>(client.GetAddressOf()));
    WAVEFORMATEX format{};
    format.wFormatTag = WAVE_FORMAT_IEEE_FLOAT;
    format.nChannels = 2;
    format.nSamplesPerSec = 48000;
    format.wBitsPerSample = 32;
    format.nBlockAlign = 8;
    format.nAvgBytesPerSec = 384000;
    UINT32 default_period = 0, fundamental = 0, minimum = 0, maximum = 0;
    if (SUCCEEDED(hr))
      hr = client->GetSharedModeEnginePeriod(&format, &default_period,
                                             &fundamental, &minimum, &maximum);
    if (SUCCEEDED(hr)) {
      period_frames_ = minimum;
      hr = client->InitializeSharedAudioStream(
          AUDCLNT_STREAMFLAGS_EVENTCALLBACK, minimum, &format, nullptr);
    }
    if (SUCCEEDED(hr)) {
      event_ = CreateEventW(nullptr, FALSE, FALSE, nullptr);
      if (!event_) hr = HRESULT_FROM_WIN32(GetLastError());
    }
    if (SUCCEEDED(hr)) hr = client->SetEventHandle(event_);
    if (SUCCEEDED(hr)) hr = client->GetService(IID_PPV_ARGS(&render));
    if (SUCCEEDED(hr)) hr = client->Start();
    FinishInitialization(hr);
    if (FAILED(hr)) {
      if (uninitialize) CoUninitialize();
      return;
    }

    while (true) {
      WaitForSingleObject(event_, INFINITE);
      {
        std::lock_guard<std::mutex> lock(mutex_);
        if (stop_) break;
      }
      UINT32 padding = 0, buffer_frames = 0;
      if (FAILED(client->GetCurrentPadding(&padding)) ||
          FAILED(client->GetBufferSize(&buffer_frames)) ||
          padding >= buffer_frames)
        continue;
      const UINT32 available = buffer_frames - padding;
      BYTE *destination = nullptr;
      if (FAILED(render->GetBuffer(available, &destination))) continue;
      float *output = reinterpret_cast<float *>(destination);
      size_t copied_frames = 0;
      {
        std::lock_guard<std::mutex> lock(mutex_);
        copied_frames = std::min<size_t>(available, queue_.size() / 2);
        for (size_t i = 0; i < copied_frames * 2; ++i) {
          output[i] = queue_.front();
          queue_.pop_front();
        }
      }
      if (copied_frames < available) {
        std::fill(output + copied_frames * 2, output + available * 2, 0.0f);
        counters_->audio_underflows.fetch_add(1, std::memory_order_relaxed);
      }
      if (SUCCEEDED(render->ReleaseBuffer(available, 0)))
        counters_->audio_rendered.fetch_add(copied_frames,
                                            std::memory_order_relaxed);
    }
    client->Stop();
    if (uninitialize) CoUninitialize();
  }

  LiveCounters *counters_;
  std::thread thread_;
  std::mutex mutex_;
  std::condition_variable ready_cv_;
  std::deque<float> queue_;
  HANDLE event_ = nullptr;
  UINT32 period_frames_ = 0;
  bool initialized_ = false;
  bool stop_ = false;
  HRESULT initialize_result_ = E_PENDING;
};

struct LiveSession {
  explicit LiveSession(HWND target) : hwnd(target), audio_renderer(&counters) {}
  HWND hwnd;
  LiveCounters counters;
  AudioRenderer audio_renderer;
  bool video_ready = false;
  bool audio_ready = false;
  HRESULT last_hresult = S_OK;
  char unavailable_reason[256]{};
  UINT32 video_width = 0;
  UINT32 video_height = 0;
  LONGLONG video_time = 0;
  LONGLONG audio_time = 0;
  ComPtr<ID3D11Device> device;
  ComPtr<ID3D11DeviceContext> context;
  ComPtr<IMFDXGIDeviceManager> manager;
  ComPtr<IMFTransform> video_decoder;
  ComPtr<IMFTransform> audio_decoder;
  ComPtr<IDXGISwapChain1> swapchain;
  ComPtr<ID3D11VideoDevice> video_device;
  ComPtr<ID3D11VideoContext> video_context;
  ComPtr<ID3D11VideoProcessorEnumerator> video_enumerator;
  ComPtr<ID3D11VideoProcessor> video_processor;
  UINT32 output_width = 0;
  UINT32 output_height = 0;
};

void LiveFailure(LiveSession *session, const char *stage, HRESULT hr) {
  session->last_hresult = hr;
  if (!session->unavailable_reason[0])
    sprintf_s(session->unavailable_reason, "%s failed (HRESULT 0x%08lx)",
              stage, static_cast<unsigned long>(hr));
}

HRESULT ConfigureLiveVideo(LiveSession *session) {
  constexpr UINT flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT |
                         D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
  constexpr D3D_FEATURE_LEVEL levels[] = {D3D_FEATURE_LEVEL_11_1,
                                           D3D_FEATURE_LEVEL_11_0,
                                           D3D_FEATURE_LEVEL_10_1,
                                           D3D_FEATURE_LEVEL_10_0};
  D3D_FEATURE_LEVEL selected{};
  HRESULT hr = D3D11CreateDevice(
      nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, flags, levels,
      static_cast<UINT>(std::size(levels)), D3D11_SDK_VERSION, &session->device,
      &selected, &session->context);
  if (hr == E_INVALIDARG)
    hr = D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, flags,
                           levels + 1, static_cast<UINT>(std::size(levels) - 1),
                           D3D11_SDK_VERSION, &session->device, &selected,
                           &session->context);
  if (FAILED(hr)) return hr;
  ComPtr<ID3D10Multithread> multithread;
  if (FAILED(hr = session->context.As(&multithread))) return hr;
  multithread->SetMultithreadProtected(TRUE);
  UINT token = 0;
  if (FAILED(hr = MFCreateDXGIDeviceManager(&token, &session->manager))) return hr;
  if (FAILED(hr = session->manager->ResetDevice(session->device.Get(), token))) return hr;
  if (FAILED(hr = CoCreateInstance(CLSID_CMSH264DecoderMFT, nullptr,
                                    CLSCTX_INPROC_SERVER,
                                    IID_PPV_ARGS(&session->video_decoder))))
    return hr;
  if (FAILED(hr = session->video_decoder->ProcessMessage(
                 MFT_MESSAGE_SET_D3D_MANAGER,
                 reinterpret_cast<ULONG_PTR>(session->manager.Get()))))
    return hr;
  ComPtr<ICodecAPI> codec;
  if (SUCCEEDED(session->video_decoder.As(&codec))) {
    VARIANT enabled;
    VariantInit(&enabled);
    enabled.vt = VT_UI4;
    enabled.ulVal = TRUE;
    codec->SetValue(&CODECAPI_AVDecVideoAcceleration_H264, &enabled);
    codec->SetValue(&CODECAPI_AVLowLatencyMode, &enabled);
    VariantClear(&enabled);
  }
  ComPtr<IMFMediaType> input;
  if (FAILED(hr = MFCreateMediaType(&input)) ||
      FAILED(hr = input->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video)) ||
      FAILED(hr = input->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_H264_ES)) ||
      FAILED(hr = session->video_decoder->SetInputType(0, input.Get(), 0)))
    return hr;
  for (DWORD index = 0;; ++index) {
    ComPtr<IMFMediaType> output;
    if (FAILED(hr = session->video_decoder->GetOutputAvailableType(
                   0, index, &output)))
      return hr;
    GUID subtype{};
    if (SUCCEEDED(output->GetGUID(MF_MT_SUBTYPE, &subtype)) &&
        subtype == MFVideoFormat_NV12) {
      if (FAILED(hr = session->video_decoder->SetOutputType(0, output.Get(), 0)))
        return hr;
      MFGetAttributeSize(output.Get(), MF_MT_FRAME_SIZE, &session->video_width,
                         &session->video_height);
      break;
    }
  }
  session->video_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
  session->video_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
  return S_OK;
}

HRESULT SelectVideoOutput(LiveSession *session) {
  for (DWORD index = 0;; ++index) {
    ComPtr<IMFMediaType> type;
    HRESULT hr = session->video_decoder->GetOutputAvailableType(0, index, &type);
    if (FAILED(hr)) return hr;
    GUID subtype{};
    if (SUCCEEDED(type->GetGUID(MF_MT_SUBTYPE, &subtype)) &&
        subtype == MFVideoFormat_NV12) {
      if (FAILED(hr = session->video_decoder->SetOutputType(0, type.Get(), 0)))
        return hr;
      MFGetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, &session->video_width,
                         &session->video_height);
      return S_OK;
    }
  }
}

HRESULT EnsureSwapchain(LiveSession *session, UINT source_width,
                        UINT source_height) {
  RECT client{};
  if (!GetClientRect(session->hwnd, &client))
    return HRESULT_FROM_WIN32(GetLastError());
  const UINT width = std::max<LONG>(1, client.right - client.left);
  const UINT height = std::max<LONG>(1, client.bottom - client.top);
  if (session->swapchain && session->output_width == width &&
      session->output_height == height && session->video_width == source_width &&
      session->video_height == source_height)
    return S_OK;
  session->video_enumerator.Reset();
  session->video_processor.Reset();
  if (session->swapchain) {
    session->swapchain.Reset();
  }
  ComPtr<IDXGIDevice> dxgi_device;
  ComPtr<IDXGIAdapter> adapter;
  ComPtr<IDXGIFactory2> factory;
  HRESULT hr = session->device.As(&dxgi_device);
  if (FAILED(hr) || FAILED(hr = dxgi_device->GetAdapter(&adapter)) ||
      FAILED(hr = adapter->GetParent(IID_PPV_ARGS(&factory))))
    return hr;
  DXGI_SWAP_CHAIN_DESC1 desc{};
  desc.Width = width;
  desc.Height = height;
  desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
  desc.SampleDesc.Count = 1;
  desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
  desc.BufferCount = 2;
  desc.SwapEffect = DXGI_SWAP_EFFECT_FLIP_DISCARD;
  desc.Scaling = DXGI_SCALING_STRETCH;
  desc.Flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;
  if (FAILED(hr = factory->CreateSwapChainForHwnd(session->device.Get(),
                                                   session->hwnd, &desc, nullptr,
                                                   nullptr, &session->swapchain)))
    return hr;
  ComPtr<IDXGISwapChain2> swapchain2;
  if (SUCCEEDED(session->swapchain.As(&swapchain2)))
    swapchain2->SetMaximumFrameLatency(1);
  if (FAILED(hr = session->device.As(&session->video_device)) ||
      FAILED(hr = session->context.As(&session->video_context)))
    return hr;
  D3D11_VIDEO_PROCESSOR_CONTENT_DESC content{};
  content.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
  content.InputWidth = source_width;
  content.InputHeight = source_height;
  content.OutputWidth = width;
  content.OutputHeight = height;
  content.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
  if (FAILED(hr = session->video_device->CreateVideoProcessorEnumerator(
                 &content, &session->video_enumerator)) ||
      FAILED(hr = session->video_device->CreateVideoProcessor(
                 session->video_enumerator.Get(), 0,
                 &session->video_processor)))
    return hr;
  session->video_width = source_width;
  session->video_height = source_height;
  session->output_width = width;
  session->output_height = height;
  return S_OK;
}

HRESULT PresentTexture(LiveSession *session, ID3D11Texture2D *texture,
                       UINT subresource) {
  D3D11_TEXTURE2D_DESC texture_desc{};
  texture->GetDesc(&texture_desc);
  HRESULT hr = EnsureSwapchain(session, texture_desc.Width, texture_desc.Height);
  if (FAILED(hr)) return hr;
  ComPtr<ID3D11Texture2D> backbuffer;
  if (FAILED(hr = session->swapchain->GetBuffer(0, IID_PPV_ARGS(&backbuffer))))
    return hr;
  D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC input_desc{};
  input_desc.FourCC = 0;
  input_desc.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
  input_desc.Texture2D.MipSlice = 0;
  input_desc.Texture2D.ArraySlice = subresource;
  ComPtr<ID3D11VideoProcessorInputView> input_view;
  if (FAILED(hr = session->video_device->CreateVideoProcessorInputView(
                 texture, session->video_enumerator.Get(), &input_desc,
                 &input_view)))
    return hr;
  D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC output_desc{};
  output_desc.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
  output_desc.Texture2D.MipSlice = 0;
  ComPtr<ID3D11VideoProcessorOutputView> output_view;
  if (FAILED(hr = session->video_device->CreateVideoProcessorOutputView(
                 backbuffer.Get(), session->video_enumerator.Get(),
                 &output_desc, &output_view)))
    return hr;
  D3D11_VIDEO_PROCESSOR_STREAM stream{};
  stream.Enable = TRUE;
  stream.pInputSurface = input_view.Get();
  if (FAILED(hr = session->video_context->VideoProcessorBlt(
                 session->video_processor.Get(), output_view.Get(), 0, 1,
                 &stream)))
    return hr;
  return session->swapchain->Present(0, DXGI_PRESENT_DO_NOT_WAIT);
}

HRESULT ConfigureLiveAudio(LiveSession *session) {
  MFT_REGISTER_TYPE_INFO input_info{MFMediaType_Audio, MFAudioFormat_Opus};
  MFT_REGISTER_TYPE_INFO output_info{MFMediaType_Audio, MFAudioFormat_Float};
  IMFActivate **activations = nullptr;
  UINT32 count = 0;
  HRESULT hr = MFTEnumEx(MFT_CATEGORY_AUDIO_DECODER,
                         MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                         &input_info, &output_info, &activations, &count);
  if (FAILED(hr) || count == 0) {
    CoTaskMemFree(activations);
    return FAILED(hr) ? hr : MF_E_TOPO_CODEC_NOT_FOUND;
  }
  hr = activations[0]->ActivateObject(IID_PPV_ARGS(&session->audio_decoder));
  for (UINT32 i = 0; i < count; ++i) activations[i]->Release();
  CoTaskMemFree(activations);
  if (FAILED(hr)) return hr;
  ComPtr<IMFMediaType> input;
  if (FAILED(hr = MFCreateMediaType(&input)) ||
      FAILED(hr = input->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio)) ||
      FAILED(hr = input->SetGUID(MF_MT_SUBTYPE, MFAudioFormat_Opus)) ||
      FAILED(hr = input->SetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)) ||
      FAILED(hr = input->SetUINT32(MF_MT_AUDIO_NUM_CHANNELS, 2)) ||
      FAILED(hr = session->audio_decoder->SetInputType(0, input.Get(), 0)))
    return hr;
  for (DWORD index = 0;; ++index) {
    ComPtr<IMFMediaType> output;
    if (FAILED(hr = session->audio_decoder->GetOutputAvailableType(0, index,
                                                                   &output)))
      return hr;
    GUID subtype{};
    UINT32 rate = 0, channels = 0;
    output->GetGUID(MF_MT_SUBTYPE, &subtype);
    output->GetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, &rate);
    output->GetUINT32(MF_MT_AUDIO_NUM_CHANNELS, &channels);
    if (subtype == MFAudioFormat_Float && rate == 48000 && channels == 2 &&
        SUCCEEDED(session->audio_decoder->SetOutputType(0, output.Get(), 0)))
      break;
  }
  session->audio_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
  session->audio_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
  return session->audio_renderer.Start();
}

HRESULT MakeInputSample(const uint8_t *data, uint32_t size, LONGLONG time,
                        LONGLONG duration, IMFSample **result) {
  ComPtr<IMFSample> sample;
  ComPtr<IMFMediaBuffer> buffer;
  HRESULT hr = MFCreateSample(&sample);
  if (FAILED(hr) || FAILED(hr = MFCreateMemoryBuffer(size, &buffer))) return hr;
  BYTE *destination = nullptr;
  DWORD capacity = 0;
  if (FAILED(hr = buffer->Lock(&destination, &capacity, nullptr))) return hr;
  memcpy(destination, data, size);
  buffer->Unlock();
  if (FAILED(hr = buffer->SetCurrentLength(size)) ||
      FAILED(hr = sample->AddBuffer(buffer.Get())))
    return hr;
  sample->SetSampleTime(time);
  sample->SetSampleDuration(duration);
  *result = sample.Detach();
  return S_OK;
}

HRESULT SubmitLiveVideo(LiveSession *session, const uint8_t *data,
                        uint32_t size, bool keyframe) {
  session->counters.video_packets.fetch_add(1, std::memory_order_relaxed);
  ComPtr<IMFSample> input;
  HRESULT hr = MakeInputSample(data, size, session->video_time, 166667, &input);
  session->video_time += 166667;
  if (FAILED(hr)) return hr;
  if (keyframe) input->SetUINT32(MFSampleExtension_CleanPoint, TRUE);
  if (FAILED(hr = session->video_decoder->ProcessInput(0, input.Get(), 0)))
    return hr;
  for (;;) {
    MFT_OUTPUT_DATA_BUFFER output{};
    DWORD status = 0;
    hr = session->video_decoder->ProcessOutput(0, 1, &output, &status);
    if (hr == MF_E_TRANSFORM_STREAM_CHANGE) {
      if (FAILED(hr = SelectVideoOutput(session))) return hr;
      continue;
    }
    if (hr == MF_E_TRANSFORM_NEED_MORE_INPUT) return S_OK;
    if (FAILED(hr)) return hr;
    ComPtr<IMFSample> sample;
    sample.Attach(output.pSample);
    if (output.pEvents) output.pEvents->Release();
    session->counters.video_decoded.fetch_add(1, std::memory_order_relaxed);
    ComPtr<IMFMediaBuffer> buffer;
    ComPtr<IMFDXGIBuffer> dxgi_buffer;
    ComPtr<ID3D11Texture2D> texture;
    UINT subresource = 0;
    if (!sample || FAILED(hr = sample->GetBufferByIndex(0, &buffer)) ||
        FAILED(hr = buffer.As(&dxgi_buffer)) ||
        FAILED(hr = dxgi_buffer->GetResource(IID_PPV_ARGS(&texture))) ||
        FAILED(hr = dxgi_buffer->GetSubresourceIndex(&subresource)))
      return FAILED(hr) ? hr : E_NOINTERFACE;
    hr = PresentTexture(session, texture.Get(), subresource);
    if (hr == DXGI_ERROR_WAS_STILL_DRAWING) {
      session->counters.video_dropped.fetch_add(1, std::memory_order_relaxed);
      return S_OK;
    }
    if (FAILED(hr)) return hr;
    session->counters.video_presented.fetch_add(1, std::memory_order_relaxed);
  }
}

HRESULT SubmitLiveAudio(LiveSession *session, const uint8_t *data,
                        uint32_t size) {
  session->counters.audio_packets.fetch_add(1, std::memory_order_relaxed);
  ComPtr<IMFSample> input;
  HRESULT hr = MakeInputSample(data, size, session->audio_time, 200000, &input);
  session->audio_time += 200000;
  if (FAILED(hr) ||
      FAILED(hr = session->audio_decoder->ProcessInput(0, input.Get(), 0)))
    return hr;
  for (;;) {
    MFT_OUTPUT_STREAM_INFO info{};
    if (FAILED(hr = session->audio_decoder->GetOutputStreamInfo(0, &info)))
      return hr;
    ComPtr<IMFSample> output_sample;
    ComPtr<IMFMediaBuffer> output_buffer;
    if (FAILED(hr = MFCreateSample(&output_sample)) ||
        FAILED(hr = MFCreateMemoryBuffer(std::max<DWORD>(info.cbSize, 32768),
                                         &output_buffer)) ||
        FAILED(hr = output_sample->AddBuffer(output_buffer.Get())))
      return hr;
    MFT_OUTPUT_DATA_BUFFER output{};
    output.pSample = output_sample.Get();
    DWORD status = 0;
    hr = session->audio_decoder->ProcessOutput(0, 1, &output, &status);
    if (output.pEvents) output.pEvents->Release();
    if (hr == MF_E_TRANSFORM_NEED_MORE_INPUT) return S_OK;
    if (hr == MF_E_TRANSFORM_STREAM_CHANGE) continue;
    if (FAILED(hr)) return hr;
    BYTE *bytes = nullptr;
    DWORD length = 0;
    if (FAILED(hr = output_buffer->Lock(&bytes, nullptr, &length))) return hr;
    const size_t frames = length / (sizeof(float) * 2);
    session->audio_renderer.Push(reinterpret_cast<float *>(bytes), frames);
    output_buffer->Unlock();
    session->counters.audio_decoded.fetch_add(frames,
                                              std::memory_order_relaxed);
  }
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

extern "C" int32_t parsec_native_media_session_create(
    HWND hwnd, void **result, ParsecNativeMediaStats *stats) {
  if (!hwnd || !result || !stats) return static_cast<int32_t>(E_POINTER);
  *result = nullptr;
  memset(stats, 0, sizeof(*stats));
  const HRESULT com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
  if (FAILED(com) && com != RPC_E_CHANGED_MODE) return static_cast<int32_t>(com);
  const HRESULT media = MFStartup(MF_VERSION, MFSTARTUP_LITE);
  if (FAILED(media)) {
    if (SUCCEEDED(com)) CoUninitialize();
    return static_cast<int32_t>(media);
  }
  auto *session = new (std::nothrow) LiveSession(hwnd);
  if (!session) {
    MFShutdown();
    if (SUCCEEDED(com)) CoUninitialize();
    return static_cast<int32_t>(E_OUTOFMEMORY);
  }
  HRESULT video = ConfigureLiveVideo(session);
  if (SUCCEEDED(video)) {
    session->video_ready = true;
  } else {
    LiveFailure(session, "live D3D11 H.264 setup", video);
  }
  HRESULT audio = ConfigureLiveAudio(session);
  if (SUCCEEDED(audio)) {
    session->audio_ready = true;
  } else {
    LiveFailure(session, "live Opus/WASAPI setup", audio);
  }
  stats->video_ready = session->video_ready;
  stats->audio_ready = session->audio_ready;
  stats->last_hresult = static_cast<int32_t>(session->last_hresult);
  CopyText(stats->unavailable_reason, std::size(stats->unavailable_reason),
           session->unavailable_reason);
  *result = session;
  return 0;
}

extern "C" int32_t parsec_native_media_session_submit_video(
    void *opaque, const uint8_t *data, uint32_t size, int32_t keyframe) {
  if (!opaque || !data || !size) return static_cast<int32_t>(E_INVALIDARG);
  auto *session = static_cast<LiveSession *>(opaque);
  if (!session->video_ready) return static_cast<int32_t>(MF_E_NOT_INITIALIZED);
  HRESULT hr = SubmitLiveVideo(session, data, size, keyframe != 0);
  if (FAILED(hr)) LiveFailure(session, "H.264 submit/present", hr);
  return static_cast<int32_t>(hr);
}

extern "C" int32_t parsec_native_media_session_submit_audio(
    void *opaque, const uint8_t *data, uint32_t size) {
  if (!opaque || !data || !size) return static_cast<int32_t>(E_INVALIDARG);
  auto *session = static_cast<LiveSession *>(opaque);
  if (!session->audio_ready) return static_cast<int32_t>(MF_E_NOT_INITIALIZED);
  HRESULT hr = SubmitLiveAudio(session, data, size);
  if (FAILED(hr)) LiveFailure(session, "Opus submit/render", hr);
  return static_cast<int32_t>(hr);
}

extern "C" void parsec_native_media_session_stats(
    void *opaque, ParsecNativeMediaStats *stats) {
  if (!opaque || !stats) return;
  auto *session = static_cast<LiveSession *>(opaque);
  memset(stats, 0, sizeof(*stats));
  stats->video_ready = session->video_ready;
  stats->audio_ready = session->audio_ready;
  stats->video_packets_submitted = session->counters.video_packets.load();
  stats->video_frames_decoded = session->counters.video_decoded.load();
  stats->video_frames_presented = session->counters.video_presented.load();
  stats->video_frames_dropped = session->counters.video_dropped.load();
  stats->video_width = session->video_width;
  stats->video_height = session->video_height;
  stats->audio_packets_submitted = session->counters.audio_packets.load();
  stats->audio_pcm_frames_decoded = session->counters.audio_decoded.load();
  stats->audio_pcm_frames_rendered = session->counters.audio_rendered.load();
  stats->audio_frames_dropped = session->counters.audio_dropped.load();
  stats->audio_underflows = session->counters.audio_underflows.load();
  stats->last_hresult = static_cast<int32_t>(session->last_hresult);
  CopyText(stats->unavailable_reason, std::size(stats->unavailable_reason),
           session->unavailable_reason);
}

extern "C" void parsec_native_media_session_destroy(void *opaque) {
  auto *session = static_cast<LiveSession *>(opaque);
  if (!session) return;
  session->audio_renderer.Stop();
  if (session->video_decoder) {
    session->video_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
    session->video_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
  }
  if (session->audio_decoder) {
    session->audio_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
    session->audio_decoder->ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
  }
  delete session;
  MFShutdown();
  CoUninitialize();
}
