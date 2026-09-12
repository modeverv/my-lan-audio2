#include "shared.h"
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <wrl/client.h>
#include <tlhelp32.h>
#include <vector>
#include <algorithm>
#include <cstdio>
#include <cmath>
#include <string>
#include <stdexcept>
#include <avrt.h>
using Microsoft::WRL::ComPtr;
static void check(bool ok,const char* message){if(!ok)throw std::runtime_error(std::string(message)+": "+std::to_string(GetLastError()));}
static void hrcheck(HRESULT hr,const char* message){if(FAILED(hr))throw std::runtime_error(std::string(message)+": HRESULT "+std::to_string(static_cast<unsigned>(hr)));}
static ComPtr<IAudioClient3> client(){
    ComPtr<IMMDeviceEnumerator> enumerator;ComPtr<IMMDevice> endpoint;ComPtr<IAudioClient3> c;
    hrcheck(CoCreateInstance(__uuidof(MMDeviceEnumerator),nullptr,CLSCTX_ALL,IID_PPV_ARGS(&enumerator)),"enumerator");
    hrcheck(enumerator->GetDefaultAudioEndpoint(eRender,eMultimedia,&endpoint),"endpoint");
    hrcheck(endpoint->Activate(__uuidof(IAudioClient3),CLSCTX_ALL,nullptr,reinterpret_cast<void**>(c.GetAddressOf())),"client");return c;
}
static uint64_t module(DWORD pid,const wchar_t* path){
    HANDLE s=CreateToolhelp32Snapshot(TH32CS_SNAPMODULE|TH32CS_SNAPMODULE32,pid);check(s!=INVALID_HANDLE_VALUE,"module snapshot");
    MODULEENTRY32W e{sizeof(e)};uint64_t base=0;
    if(Module32FirstW(s,&e))do{if(_wcsicmp(e.szExePath,path)==0){base=reinterpret_cast<uint64_t>(e.modBaseAddr);break;}}while(Module32NextW(s,&e));
    CloseHandle(s);return base;
}
static uint64_t remoteAddress(DWORD pid,void* pointer){
    HMODULE owner=nullptr;check(GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS|GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,reinterpret_cast<LPCWSTR>(pointer),&owner),"function owner");
    wchar_t path[MAX_PATH];check(GetModuleFileNameW(owner,path,MAX_PATH)!=0,"module path");
    auto base=module(pid,path);check(base!=0,"matching target module (must already be loaded)");return base+reinterpret_cast<uint64_t>(pointer)-reinterpret_cast<uint64_t>(owner);
}
static void* copyRemote(HANDLE process,const void* data,size_t size){
    void* p=VirtualAllocEx(process,nullptr,size,MEM_COMMIT|MEM_RESERVE,PAGE_READWRITE);check(p!=nullptr,"remote argument allocation");
    SIZE_T written=0;check(WriteProcessMemory(process,p,data,size,&written)&&written==size,"remote argument write");return p;
}
static HANDLE launch(HANDLE process,uint64_t entry,void* arg){
    HANDLE t=CreateRemoteThread(process,nullptr,0,reinterpret_cast<LPTHREAD_START_ROUTINE>(entry),arg,0,nullptr);check(t!=nullptr,"start probe thread");return t;
}
static DWORD WINAPI source(void*){
    CoInitializeEx(nullptr,COINIT_MULTITHREADED);
    try{
        auto c=client();WAVEFORMATEX* w=nullptr;hrcheck(c->GetMixFormat(&w),"source format");
        hrcheck(c->Initialize(AUDCLNT_SHAREMODE_SHARED,0,0,0,w,nullptr),"source initialize");
        ComPtr<IAudioRenderClient> r;hrcheck(c->GetService(IID_PPV_ARGS(&r)),"source renderer");
        UINT32 size=0;c->GetBufferSize(&size);c->Start();double phase=0;
        for(unsigned i=0;i<1200;++i){UINT32 pad=0;c->GetCurrentPadding(&pad);UINT32 frames=size-pad;
            if(frames){BYTE* b=nullptr;hrcheck(r->GetBuffer(frames,&b),"source buffer");
                for(unsigned f=0;f<frames;++f){const float v=static_cast<float>(sin(phase)*0.005);phase+=6.283185307*440/w->nSamplesPerSec;
                    for(unsigned ch=0;ch<w->nChannels;++ch)reinterpret_cast<float*>(b)[f*w->nChannels+ch]=v;}
                r->ReleaseBuffer(frames,0);}
            Sleep(5);
        }c->Stop();CoTaskMemFree(w);
    }catch(const std::exception& e){fprintf(stderr,"self source error: %s\n",e.what());}
    CoUninitialize();return 0;
}
static void stats(const char* name,std::vector<double> values){
    printf("\"%s\":",name);if(values.empty()){printf("null");return;}
    std::sort(values.begin(),values.end());auto q=[&](double p){return values[static_cast<size_t>(ceil(values.size()*p))-1];};
    printf("{\"count\":%zu,\"p50\":%.3f,\"p99\":%.3f,\"max\":%.3f}",values.size(),q(.5),q(.99),values.back());
}
int wmain(int argc,wchar_t** argv){
    if(argc!=4){fprintf(stderr,"usage: render-probe.exe PID seconds DLL_PATH (PID 0 = owned self-test, seconds >= 8)\n");return 2;}
    const DWORD requested=wcstoul(argv[1],nullptr,10),pid=requested?requested:GetCurrentProcessId();const unsigned seconds=wcstoul(argv[2],nullptr,10);
    if(seconds<1||seconds>600)return 2;
    try{
        hrcheck(CoInitializeEx(nullptr,COINIT_MULTITHREADED),"COM");
        auto c=client();WAVEFORMATEX* w=nullptr;hrcheck(c->GetMixFormat(&w),"format");
        hrcheck(c->Initialize(AUDCLNT_SHAREMODE_SHARED,0,0,0,w,nullptr),"dummy initialize");CoTaskMemFree(w);
        ComPtr<IAudioRenderClient> r;hrcheck(c->GetService(IID_PPV_ARGS(&r)),"dummy render");
        auto cv=*reinterpret_cast<void***>(c.Get());auto rv=*reinterpret_cast<void***>(r.Get());void* functions[]={cv[3],cv[20],cv[14],rv[3],rv[4]};
        HANDLE process=OpenProcess(PROCESS_CREATE_THREAD|PROCESS_QUERY_INFORMATION|PROCESS_VM_OPERATION|PROCESS_VM_WRITE|PROCESS_VM_READ|PROCESS_DUP_HANDLE|SYNCHRONIZE,FALSE,pid);check(process!=nullptr,"open target");
        HANDLE mapping=CreateFileMappingW(INVALID_HANDLE_VALUE,nullptr,PAGE_READWRITE,0,sizeof(Shared),nullptr);check(mapping!=nullptr,"mapping");
        auto* shared=static_cast<Shared*>(MapViewOfFile(mapping,FILE_MAP_ALL_ACCESS,0,0,sizeof(Shared)));check(shared!=nullptr,"mapping view");memset(shared,0,sizeof(Shared));
        HANDLE event=CreateEventW(nullptr,FALSE,FALSE,nullptr);check(event!=nullptr,"event");
        Config cfg{};cfg.seconds=seconds;
        for(unsigned i=0;i<5;++i)cfg.functions[i]=remoteAddress(pid,functions[i]);
        wchar_t dll[MAX_PATH];check(GetFullPathNameW(argv[3],MAX_PATH,dll,nullptr)!=0,"DLL path");
        HMODULE local=LoadLibraryW(dll);check(local!=nullptr,"local DLL load");
        if(requested){
            void* path=copyRemote(process,dll,(wcslen(dll)+1)*sizeof(wchar_t));
            auto loader=launch(process,remoteAddress(pid,reinterpret_cast<void*>(LoadLibraryW)),path);
            check(WaitForSingleObject(loader,15000)==WAIT_OBJECT_0,"DLL load timeout");DWORD status;GetExitCodeThread(loader,&status);CloseHandle(loader);VirtualFreeEx(process,path,0,MEM_RELEASE);
            check(status!=0,"target DLL load rejected");
        }
        check(DuplicateHandle(GetCurrentProcess(),mapping,process,&cfg.mapping,0,FALSE,DUPLICATE_SAME_ACCESS),"mapping handle transfer");
        check(DuplicateHandle(GetCurrentProcess(),event,process,&cfg.event,0,FALSE,DUPLICATE_SAME_ACCESS),"event handle transfer");
        auto entry=GetProcAddress(local,"RunProbe");check(entry!=nullptr,"probe export");
        void* argument=copyRemote(process,&cfg,sizeof(cfg));auto worker=launch(process,remoteAddress(pid,reinterpret_cast<void*>(entry)),argument);
        LARGE_INTEGER frequency;QueryPerformanceFrequency(&frequency);auto us=[&](uint64_t a,uint64_t b){return double(a-b)*1e6/frequency.QuadPart;};
        fprintf(stderr,"probe waiting pid=%lu seconds=%u; resume playback after installed\n",pid,seconds);
        struct Observation {uint64_t entry,received,stream;unsigned frames,flags;Format format;double peak,delay;long result;};
        std::vector<Observation> observations;observations.reserve(seconds*2000u);
        std::vector<double> delivery,copy,releaseTime,intervals,framesPerBlock;
        for(auto* v:{&delivery,&copy,&releaseTime,&intervals,&framesPerBlock})v->reserve(seconds*2000u);
        DWORD taskIndex=0;HANDLE mmcss=AvSetMmThreadCharacteristicsW(L"Pro Audio",&taskIndex);
        unsigned tail=0,nonzero=0,packets=0,failed=0;double maxPeak=0;uint64_t previous=0,totalFrames=0;bool announced=false;HANDLE selfSource=nullptr;
        auto start=GetTickCount64();
        while(GetTickCount64()-start<uint64_t(seconds+20)*1000){
            WaitForSingleObject(event,100);
            if(shared->installed&&!announced){fprintf(stderr,"installed: start/resume target playback now\n");announced=true;if(!requested)selfSource=CreateThread(nullptr,0,source,nullptr,0,nullptr);}
            while(InterlockedCompareExchange(&shared->packets[tail%Slots].ready,0,0)==1){
                auto& p=shared->packets[tail%Slots];auto received=tick();double peak=0;
                if(p.format.tag==WAVE_FORMAT_IEEE_FLOAT&&p.format.bits==32){for(unsigned i=0;i<p.bytes/4;++i)peak=(std::max)(peak,static_cast<double>(fabs(reinterpret_cast<float*>(p.pcm)[i])));}
                else if(p.format.tag==WAVE_FORMAT_PCM&&p.format.bits==16){for(unsigned i=0;i<p.bytes/2;++i)peak=(std::max)(peak,fabs(reinterpret_cast<short*>(p.pcm)[i]/32768.0));}
                if(peak>1e-6)++nonzero;maxPeak=(std::max)(peak,maxPeak);++packets;totalFrames+=p.frames;if(FAILED(p.result))++failed;
                delivery.push_back(us(received,p.entry));copy.push_back(us(p.copied,p.entry));releaseTime.push_back(us(p.released,p.copied));framesPerBlock.push_back(p.frames);
                if(previous)intervals.push_back(us(p.entry,previous));previous=p.entry;
                observations.push_back({p.entry,received,p.stream,p.frames,p.flags,p.format,peak,delivery.back(),p.result});
                InterlockedExchange(&p.ready,0);++tail;
            }
            if(shared->finished||WaitForSingleObject(worker,0)==WAIT_OBJECT_0)break;
        }
        if(mmcss)AvRevertMmThreadCharacteristics(mmcss);
        for(const auto& p:observations)printf("{\"event\":\"render_tap\",\"entry_qpc\":%llu,\"received_qpc\":%llu,\"stream\":%llu,\"frames\":%u,\"rate\":%u,\"channels\":%u,\"bits\":%u,\"format_tag\":%u,\"flags\":%u,\"peak\":%.8f,\"release_hr\":%ld,\"delivery_us\":%.3f}\n",p.entry,p.received,p.stream,p.frames,p.format.rate,p.format.channels,p.format.bits,p.format.tag,p.flags,p.peak,p.result,p.delay);
        printf("{\"event\":\"tap_summary\",\"pid\":%lu,\"installed\":%s,\"finished\":%s,\"error\":%ld,\"calls\":%ld,\"packets\":%u,\"frames\":%llu,\"nonzero\":%u,\"peak_max\":%.8f,\"release_failures\":%u,\"unknown_format_or_buffer\":%ld,\"dropped\":%ld,\"producer_busy\":%ld,\"qpc_frequency\":%lld,\"collector_mmcss\":%s,\"io_after_capture\":true,",pid,shared->installed?"true":"false",shared->finished?"true":"false",shared->error,shared->calls,packets,totalFrames,nonzero,maxPeak,failed,shared->unknown,shared->dropped,shared->busy,frequency.QuadPart,mmcss?"true":"false");
        stats("entry_to_collector_us",delivery);printf(",");stats("entry_to_copy_us",copy);printf(",");stats("original_release_call_us",releaseTime);printf(",");stats("release_interval_us",intervals);printf(",");stats("frames_per_block",framesPerBlock);printf("}\n");
        bool success=shared->installed&&shared->finished&&!shared->error&&packets>0;
        if(selfSource){WaitForSingleObject(selfSource,10000);CloseHandle(selfSource);}
        if(WaitForSingleObject(worker,1000)==WAIT_OBJECT_0)VirtualFreeEx(process,argument,0,MEM_RELEASE);
        CloseHandle(worker);CloseHandle(event);UnmapViewOfFile(shared);CloseHandle(mapping);CloseHandle(process);return success?0:1;
    }catch(const std::exception& e){fprintf(stderr,"%s\n",e.what());return 1;}
}
