// Diagnostic PCM tee at the WASAPI render boundary. No PCM file/network output.
#include "shared.h"
#include <audioclient.h>
#include <ksmedia.h>
#include <tlhelp32.h>
#include <detours.h>
#include <atomic>
#include <cstring>
using Init=HRESULT(STDMETHODCALLTYPE*)(IAudioClient*,AUDCLNT_SHAREMODE,DWORD,REFERENCE_TIME,REFERENCE_TIME,const WAVEFORMATEX*,LPCGUID);
using Init3=HRESULT(STDMETHODCALLTYPE*)(IAudioClient3*,DWORD,UINT32,const WAVEFORMATEX*,LPCGUID);
using Service=HRESULT(STDMETHODCALLTYPE*)(IAudioClient*,REFIID,void**);
using Get=HRESULT(STDMETHODCALLTYPE*)(IAudioRenderClient*,UINT32,BYTE**);
using Release=HRESULT(STDMETHODCALLTYPE*)(IAudioRenderClient*,UINT32,DWORD);
static Init realInit;static Init3 realInit3;static Service realService;static Get realGet;static Release realRelease;
static SRWLOCK tableLock=SRWLOCK_INIT,producerLock=SRWLOCK_INIT;
struct Entry {void* object;Format format;};static Entry formats[256];
static Shared* shared;static HANDLE notify;static unsigned head;
static std::atomic<bool> enabled=false;static std::atomic<unsigned> active=0;
static bool installed=false;
struct Pending {void* object;BYTE* data;unsigned frames;};static thread_local Pending pending{};
static Format describe(const WAVEFORMATEX* w) {
    Format f{w->nSamplesPerSec,w->nChannels,w->nBlockAlign,w->wBitsPerSample,w->wFormatTag};
    if(w->wFormatTag==WAVE_FORMAT_EXTENSIBLE && w->cbSize>=22){
        const auto* x=reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(w);
        if(IsEqualGUID(x->SubFormat,KSDATAFORMAT_SUBTYPE_IEEE_FLOAT))f.tag=WAVE_FORMAT_IEEE_FLOAT;
        else if(IsEqualGUID(x->SubFormat,KSDATAFORMAT_SUBTYPE_PCM))f.tag=WAVE_FORMAT_PCM;
    }return f;
}
static void remember(void* object,Format format){
    if(!TryAcquireSRWLockExclusive(&tableLock))return;
    Entry* slot=nullptr;
    for(auto& e:formats){if(e.object==object){slot=&e;break;}if(!e.object&&!slot)slot=&e;}
    if(slot)*slot={object,format};
    ReleaseSRWLockExclusive(&tableLock);
}
static Format lookup(void* object){
    Format f{};if(!TryAcquireSRWLockShared(&tableLock))return f;
    for(auto& e:formats)if(e.object==object){f=e.format;break;}
    ReleaseSRWLockShared(&tableLock);return f;
}
static HRESULT STDMETHODCALLTYPE init(IAudioClient* p,AUDCLNT_SHAREMODE m,DWORD flags,REFERENCE_TIME a,REFERENCE_TIME b,const WAVEFORMATEX* w,LPCGUID id){
    auto hr=realInit(p,m,flags,a,b,w,id);if(SUCCEEDED(hr)&&w)remember(p,describe(w));return hr;
}
static HRESULT STDMETHODCALLTYPE init3(IAudioClient3* p,DWORD flags,UINT32 frames,const WAVEFORMATEX* w,LPCGUID id){
    auto hr=realInit3(p,flags,frames,w,id);if(SUCCEEDED(hr)&&w)remember(p,describe(w));return hr;
}
static HRESULT STDMETHODCALLTYPE service(IAudioClient* p,REFIID id,void** result){
    auto hr=realService(p,id,result);
    if(SUCCEEDED(hr)&&IsEqualIID(id,__uuidof(IAudioRenderClient))&&result)remember(*result,lookup(p));return hr;
}
static HRESULT STDMETHODCALLTYPE get(IAudioRenderClient* p,UINT32 frames,BYTE** data){
    auto hr=realGet(p,frames,data);if(SUCCEEDED(hr)&&frames&&data)pending={p,*data,frames};return hr;
}
static HRESULT STDMETHODCALLTYPE release(IAudioRenderClient* p,UINT32 frames,DWORD flags){
    active.fetch_add(1);Packet* packet=nullptr;const auto entry=tick();bool held=false;
    if(enabled.load()){
        InterlockedIncrement(&shared->calls);
        const auto f=lookup(p);const uint64_t bytes=uint64_t(frames)*f.align;
        if(!f.align||pending.object!=p||frames>pending.frames){InterlockedIncrement(&shared->unknown);}
        else if(bytes>MaxBytes){InterlockedIncrement(&shared->dropped);}
        else if(!TryAcquireSRWLockExclusive(&producerLock)){InterlockedIncrement(&shared->busy);}
        else {
            held=true;auto& s=shared->packets[head%Slots];
            if(InterlockedCompareExchange(&s.ready,0,0)!=0)InterlockedIncrement(&shared->dropped);
            else {
                s.entry=entry;s.frames=frames;s.flags=flags;s.bytes=static_cast<unsigned>(bytes);s.format=f;s.stream=reinterpret_cast<uint64_t>(p);
                if(flags&AUDCLNT_BUFFERFLAGS_SILENT)memset(s.pcm,0,s.bytes);
                else if(s.bytes)memcpy(s.pcm,pending.data,s.bytes);
                s.copied=tick();packet=&s;
            }
        }
    }
    pending={};auto hr=realRelease(p,frames,flags);
    if(packet){packet->result=hr;packet->released=tick();InterlockedExchange(&packet->ready,1);++head;SetEvent(notify);}
    if(held)ReleaseSRWLockExclusive(&producerLock);
    active.fetch_sub(1);return hr;
}
// Hooks remain resident but disabled after the finite experiment. No unsafe DLL unload
// while another app thread may still be returning through a detour trampoline.
extern "C" __declspec(dllexport) DWORD WINAPI RunProbe(void* input){
    const auto cfg=*static_cast<Config*>(input);
    if(enabled.load())return ERROR_BUSY;
    shared=static_cast<Shared*>(MapViewOfFile(cfg.mapping,FILE_MAP_ALL_ACCESS,0,0,sizeof(Shared)));
    if(!shared)return GetLastError();notify=cfg.event;head=0;
    if(!installed){
        realInit=reinterpret_cast<Init>(cfg.functions[0]);realInit3=reinterpret_cast<Init3>(cfg.functions[1]);
        realService=reinterpret_cast<Service>(cfg.functions[2]);realGet=reinterpret_cast<Get>(cfg.functions[3]);realRelease=reinterpret_cast<Release>(cfg.functions[4]);
        auto error=DetourTransactionBegin();
        HANDLE threads[2048]{};unsigned count=0;
        auto snapshot=CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD,0);THREADENTRY32 e{sizeof(e)};
        if(snapshot==INVALID_HANDLE_VALUE)error=GetLastError();
        else if(Thread32First(snapshot,&e))do{
            if(e.th32OwnerProcessID==GetCurrentProcessId()&&e.th32ThreadID!=GetCurrentThreadId()){
                if(count==2048){error=ERROR_TOO_MANY_TCBS;break;}
                HANDLE h=OpenThread(THREAD_SUSPEND_RESUME|THREAD_GET_CONTEXT|THREAD_SET_CONTEXT|THREAD_QUERY_INFORMATION,FALSE,e.th32ThreadID);
                if(!h){error=GetLastError();break;}
                threads[count++]=h;auto status=DetourUpdateThread(h);if(status){error=status;break;}
            }
        }while(Thread32Next(snapshot,&e));
        if(snapshot!=INVALID_HANDLE_VALUE)CloseHandle(snapshot);
        if(!error)error=DetourAttach(reinterpret_cast<PVOID*>(&realInit),init);
        if(!error)error=DetourAttach(reinterpret_cast<PVOID*>(&realInit3),init3);
        if(!error)error=DetourAttach(reinterpret_cast<PVOID*>(&realService),service);
        if(!error)error=DetourAttach(reinterpret_cast<PVOID*>(&realGet),get);
        if(!error)error=DetourAttach(reinterpret_cast<PVOID*>(&realRelease),release);
        if(error)DetourTransactionAbort();else error=DetourTransactionCommit();
        for(unsigned i=0;i<count;++i)CloseHandle(threads[i]);
        if(error){shared->error=error;InterlockedExchange(&shared->finished,1);UnmapViewOfFile(shared);CloseHandle(cfg.mapping);CloseHandle(cfg.event);return error;}
        installed=true;
    }
    enabled.store(true);InterlockedExchange(&shared->installed,1);SetEvent(notify);
    Sleep(cfg.seconds*1000);enabled.store(false);
    while(active.load())Sleep(1);
    InterlockedExchange(&shared->finished,1);SetEvent(notify);
    UnmapViewOfFile(shared);CloseHandle(cfg.mapping);CloseHandle(cfg.event);return 0;
}
BOOL WINAPI DllMain(HINSTANCE,DWORD,LPVOID){return TRUE;}
