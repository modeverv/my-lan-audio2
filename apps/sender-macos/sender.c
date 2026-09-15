#include "sender.h"
#include <AudioToolbox/AudioToolbox.h>
#include <CoreAudio/HostTime.h>
#include <CoreFoundation/CoreFoundation.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <math.h>
#include <stdatomic.h>

#define MAX_FRAMES 8192
struct Sender {
    AudioUnit unit;
    int socket;
    uint32_t rate;
    uint64_t session, sequence, first;
    double expected_sample;
    int has_sample;
    float pcm[MAX_FRAMES * 2];
    _Atomic uint64_t packets, frames, send_errors, capture_errors;
    _Atomic float peak;
    _Atomic int last_error;
};
static OSStatus get(AudioObjectID id, UInt32 selector, UInt32 scope, void *value, UInt32 *size) {
    AudioObjectPropertyAddress a={selector,scope,kAudioObjectPropertyElementMain};
    return AudioObjectGetPropertyData(id,&a,0,NULL,size,value);
}
int sender_devices(SenderDevice *out,int capacity) {
    AudioObjectPropertyAddress a={kAudioHardwarePropertyDevices,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
    UInt32 bytes=0;
    if(AudioObjectGetPropertyDataSize(kAudioObjectSystemObject,&a,0,NULL,&bytes))return 0;
    AudioDeviceID *ids=malloc(bytes);if(!ids)return 0;
    if(AudioObjectGetPropertyData(kAudioObjectSystemObject,&a,0,NULL,&bytes,ids)){free(ids);return 0;}
    int count=0;
    for(unsigned i=0;i<bytes/sizeof(*ids)&&count<capacity;i++) {
        AudioObjectPropertyAddress c={kAudioDevicePropertyStreamConfiguration,kAudioDevicePropertyScopeInput,kAudioObjectPropertyElementMain};
        UInt32 n=0;
        if(AudioObjectGetPropertyDataSize(ids[i],&c,0,NULL,&n)||n<sizeof(AudioBufferList))continue;
        AudioBufferList *b=malloc(n);if(!b)continue;
        unsigned channels=0;
        if(!AudioObjectGetPropertyData(ids[i],&c,0,NULL,&n,b))for(unsigned j=0;j<b->mNumberBuffers;j++)channels+=b->mBuffers[j].mNumberChannels;
        free(b);if(!channels)continue;
        SenderDevice *d=&out[count++];memset(d,0,sizeof(*d));d->id=ids[i];d->channels=channels;
        CFStringRef value=NULL;n=sizeof(value);
        if(!get(ids[i],kAudioObjectPropertyName,kAudioObjectPropertyScopeGlobal,&value,&n)&&value){CFStringGetCString(value,d->name,sizeof(d->name),kCFStringEncodingUTF8);CFRelease(value);}
        value=NULL;n=sizeof(value);
        if(!get(ids[i],kAudioDevicePropertyDeviceUID,kAudioObjectPropertyScopeGlobal,&value,&n)&&value){CFStringGetCString(value,d->uid,sizeof(d->uid),kCFStringEncodingUTF8);CFRelease(value);}
        n=sizeof(d->rate);get(ids[i],kAudioDevicePropertyNominalSampleRate,kAudioObjectPropertyScopeGlobal,&d->rate,&n);
        n=sizeof(d->buffer_frames);get(ids[i],kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,&d->buffer_frames,&n);
    }
    free(ids);return count;
}
static uint64_t stamp(void){return AudioConvertHostTimeToNanos(AudioGetCurrentHostTime())/100;}
static void put(unsigned char *b,unsigned offset,uint64_t v,unsigned bytes){for(unsigned i=0;i<bytes;i++)b[offset+i]=(unsigned char)(v>>(i*8));}
static void publish(Sender *s,const float *pcm,unsigned frames,uint64_t acquired) {
    float peak=0;
    for(unsigned offset=0;offset<frames;) {
        unsigned n=frames-offset;if(n>128)n=128;
        unsigned char packet[72+128*8]={0};
        memcpy(packet,"LNAU",4);put(packet,4,1,2);put(packet,6,72,2);put(packet,8,1,4);
        unsigned flags=s->sequence==0?2:0;int silent=1;
        for(unsigned i=0;i<n*2;i++){float v=fabsf(pcm[offset*2+i]);if(v>0)silent=0;if(v>peak)peak=v;}
        if(silent)flags|=1;
        put(packet,12,flags,2);packet[14]=2;packet[15]=1;
        put(packet,16,s->session,8);put(packet,24,s->sequence++,8);put(packet,32,s->first,8);
        put(packet,40,s->rate,4);put(packet,44,n,2);put(packet,48,acquired,8);
        // macOS targets are little endian, including Float32 PCM.
        memcpy(packet+72,pcm+offset*2,n*8);put(packet,56,stamp(),8);put(packet,64,stamp(),8);
        if(send(s->socket,packet,72+n*8,0)!=(ssize_t)(72+n*8)) {
            atomic_fetch_add(&s->send_errors,1);atomic_store(&s->last_error,errno);
        } else atomic_fetch_add(&s->packets,1);
        s->first+=n;offset+=n;
    }
    atomic_fetch_add(&s->frames,frames);atomic_store(&s->peak,peak);
}
static OSStatus capture(void *ctx,AudioUnitRenderActionFlags *flags,const AudioTimeStamp *time,UInt32 bus,UInt32 frames,AudioBufferList *unused) {
    (void)bus;(void)unused;Sender *s=ctx;
    if(frames>MAX_FRAMES){atomic_fetch_add(&s->capture_errors,1);s->has_sample=0;return kAudio_ParamError;}
    AudioBufferList data={.mNumberBuffers=1,.mBuffers={{2,frames*8,s->pcm}}};
    OSStatus status=AudioUnitRender(s->unit,flags,time,1,frames,&data);
    if(status){atomic_fetch_add(&s->capture_errors,1);atomic_store(&s->last_error,status);s->has_sample=0;return status;}
    if(!s->has_sample || ((time->mFlags&kAudioTimeStampSampleTimeValid)&&fabs(time->mSampleTime-s->expected_sample)>0.5)) {
        s->session++;s->sequence=0;s->first=0;
    }
    s->has_sample=1;s->expected_sample=time->mSampleTime+frames;
    publish(s,s->pcm,frames,stamp());return noErr;
}
static Sender *create(const char *host,uint16_t port,const char *interface_ip,int *error) {
    Sender *s=calloc(1,sizeof(*s));if(!s){*error=ENOMEM;return NULL;}s->socket=-1;
    struct sockaddr_in dest={.sin_len=sizeof(dest),.sin_family=AF_INET,.sin_port=htons(port)};
    if(!port||inet_pton(AF_INET,host,&dest.sin_addr)!=1){*error=EINVAL;goto fail;}
    s->socket=socket(AF_INET,SOCK_DGRAM,0);if(s->socket<0){*error=errno;goto fail;}
    if(fcntl(s->socket,F_SETFL,O_NONBLOCK)<0){*error=errno;goto fail;}
    if(interface_ip && *interface_ip) {
        struct in_addr local;
        if(inet_pton(AF_INET,interface_ip,&local)!=1){*error=EINVAL;goto fail;}
        if(setsockopt(s->socket,IPPROTO_IP,IP_MULTICAST_IF,&local,sizeof(local))){*error=errno;goto fail;}
    }
    unsigned char ttl=1;
    if(setsockopt(s->socket,IPPROTO_IP,IP_MULTICAST_TTL,&ttl,sizeof(ttl))){*error=errno;goto fail;}
    if(connect(s->socket,(struct sockaddr*)&dest,sizeof(dest))){*error=errno;goto fail;}
    arc4random_buf(&s->session,sizeof(s->session));*error=0;return s;
fail:sender_stop(s);return NULL;
}
void sender_stop(Sender *s) {
    if(!s)return;
    if(s->unit){AudioOutputUnitStop(s->unit);AudioUnitUninitialize(s->unit);AudioComponentInstanceDispose(s->unit);}
    if(s->socket>=0)close(s->socket);free(s);
}
Sender *sender_start(uint32_t device,uint32_t channel,const char *host,uint16_t port,const char *interface_ip,int *error) {
    SenderDevice devices[128];int count=sender_devices(devices,128);SenderDevice *d=NULL;
    for(int i=0;i<count;i++)if(devices[i].id==device)d=&devices[i];
    if(!d||channel>=d->channels||(d->channels>1&&channel+1>=d->channels)||d->rate<8000||d->rate>192000){*error=EINVAL;return NULL;}
    Sender *s=create(host,port,interface_ip,error);if(!s)return NULL;s->rate=(uint32_t)d->rate;
    OSStatus status=0;
#define CHECK(call) do {status=(call);if(status)goto fail;} while(0)
    AudioComponentDescription desc={kAudioUnitType_Output,kAudioUnitSubType_HALOutput,kAudioUnitManufacturer_Apple,0,0};
    AudioComponent component=AudioComponentFindNext(NULL,&desc);if(!component){status=-1;goto fail;}
    CHECK(AudioComponentInstanceNew(component,&s->unit));
    UInt32 yes=1,no=0;
    CHECK(AudioUnitSetProperty(s->unit,kAudioOutputUnitProperty_EnableIO,kAudioUnitScope_Input,1,&yes,sizeof(yes)));
    CHECK(AudioUnitSetProperty(s->unit,kAudioOutputUnitProperty_EnableIO,kAudioUnitScope_Output,0,&no,sizeof(no)));
    CHECK(AudioUnitSetProperty(s->unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&device,sizeof(device)));
    AudioStreamBasicDescription format={.mSampleRate=d->rate,.mFormatID=kAudioFormatLinearPCM,.mFormatFlags=kAudioFormatFlagIsFloat|kAudioFormatFlagIsPacked,.mBytesPerPacket=8,.mFramesPerPacket=1,.mBytesPerFrame=8,.mChannelsPerFrame=2,.mBitsPerChannel=32};
    CHECK(AudioUnitSetProperty(s->unit,kAudioUnitProperty_StreamFormat,kAudioUnitScope_Output,1,&format,sizeof(format)));
    SInt32 map[2]={(SInt32)channel,(SInt32)(d->channels==1?channel:channel+1)};
    CHECK(AudioUnitSetProperty(s->unit,kAudioOutputUnitProperty_ChannelMap,kAudioUnitScope_Output,1,map,sizeof(map)));
    UInt32 maximum=MAX_FRAMES;
    CHECK(AudioUnitSetProperty(s->unit,kAudioUnitProperty_MaximumFramesPerSlice,kAudioUnitScope_Global,0,&maximum,sizeof(maximum)));
    AURenderCallbackStruct cb={capture,s};
    CHECK(AudioUnitSetProperty(s->unit,kAudioOutputUnitProperty_SetInputCallback,kAudioUnitScope_Global,0,&cb,sizeof(cb)));
    CHECK(AudioUnitInitialize(s->unit));CHECK(AudioOutputUnitStart(s->unit));*error=0;return s;
fail:*error=status;sender_stop(s);return NULL;
}
void sender_stats(Sender *s,SenderStats *out) {
    out->packets=atomic_load(&s->packets);out->frames=atomic_load(&s->frames);
    out->send_errors=atomic_load(&s->send_errors);out->capture_errors=atomic_load(&s->capture_errors);
    out->peak=atomic_exchange(&s->peak,0);out->last_error=atomic_load(&s->last_error);
}
int sender_test(const char *host,uint16_t port) {
    int error=0;Sender *s=create(host,port,"",&error);if(!s)return error;
    s->rate=48000;float pcm[300*2];
    for(unsigned i=0;i<300;i++){pcm[i*2]=0.25f;pcm[i*2+1]=-0.5f;}
    publish(s,pcm,300,stamp());memset(pcm,0,sizeof(pcm));publish(s,pcm,1,stamp());
    error=(int)atomic_load(&s->send_errors);sender_stop(s);return error;
}
