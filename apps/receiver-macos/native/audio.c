#include <AudioToolbox/AudioToolbox.h>
#include <CoreAudio/CoreAudio.h>
#include <CoreAudio/HostTime.h>
#include <CoreFoundation/CoreFoundation.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <pthread/qos.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/thread_policy.h>
#include <pthread.h>
// This worker blocks in recvmsg between arrivals. No spin loop or periodic polling.
// Nonperiodic deadline: 0.5ms computation, 1ms constraint (Mach absolute units).
typedef struct { int qos_status, set_status, get_status, is_default; unsigned period, computation, constraint, numer, denom; } Scheduling;
void lan_network_priority(int realtime, Scheduling *report) {
    memset(report,0,sizeof(*report));
    pthread_setname_np("LAN audio UDP receive");
    report->qos_status=pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE,0);
    mach_timebase_info_data_t timebase;mach_timebase_info(&timebase);
    report->numer=timebase.numer;report->denom=timebase.denom;
    thread_port_t thread=pthread_mach_thread_np(pthread_self());
    if(realtime) {
        thread_time_constraint_policy_data_t policy={0};
        policy.period=0; // packet arrivals are asynchronous to CoreAudio
        policy.computation=(uint32_t)(500000ull*timebase.denom/timebase.numer);
        policy.constraint=(uint32_t)(1000000ull*timebase.denom/timebase.numer);
        policy.preemptible=TRUE;
        report->set_status=thread_policy_set(thread,THREAD_TIME_CONSTRAINT_POLICY,(thread_policy_t)&policy,THREAD_TIME_CONSTRAINT_POLICY_COUNT);
    }
    thread_time_constraint_policy_data_t actual={0};
    mach_msg_type_number_t count=THREAD_TIME_CONSTRAINT_POLICY_COUNT;
    boolean_t is_default=FALSE;
    report->get_status=thread_policy_get(thread,THREAD_TIME_CONSTRAINT_POLICY,(thread_policy_t)&actual,&count,&is_default);
    report->is_default=is_default;report->period=actual.period;
    report->computation=actual.computation;report->constraint=actual.constraint;
}

typedef void (*Render)(void *, float *, unsigned, double);
typedef struct { unsigned id, channels, buffer_frames, latency_frames, safety_frames; double rate; char name[256]; } Device;
typedef struct { AudioUnit unit; Render render; void *context; AudioDeviceID device; UInt32 original_buffer, requested_buffer; } Output;
static OSStatus get(AudioObjectID id, UInt32 sel, UInt32 scope, void *data, UInt32 *size) {
    AudioObjectPropertyAddress a={sel,scope,kAudioObjectPropertyElementMain};
    return AudioObjectGetPropertyData(id,&a,0,NULL,size,data);
}
static OSStatus set(AudioObjectID id, UInt32 sel, const void *data, UInt32 size) {
    AudioObjectPropertyAddress a={sel,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
    return AudioObjectSetPropertyData(id,&a,0,NULL,size,data);
}
static unsigned uintprop(AudioObjectID id,UInt32 sel,UInt32 scope) {
    UInt32 v=0,n=sizeof(v);get(id,sel,scope,&v,&n);return v;
}
int lan_devices(Device *out,int capacity) {
    AudioObjectPropertyAddress a={kAudioHardwarePropertyDevices,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
    UInt32 size=0;
    if(AudioObjectGetPropertyDataSize(kAudioObjectSystemObject,&a,0,NULL,&size))return -1;
    AudioDeviceID *ids=malloc(size);
    if(!ids)return -1;
    if(AudioObjectGetPropertyData(kAudioObjectSystemObject,&a,0,NULL,&size,ids)){free(ids);return -1;}
    int count=0;
    for(unsigned i=0;i<size/sizeof(*ids)&&count<capacity;i++) {
        AudioObjectPropertyAddress c={kAudioDevicePropertyStreamConfiguration,kAudioDevicePropertyScopeOutput,kAudioObjectPropertyElementMain};
        UInt32 n=0;
        if(AudioObjectGetPropertyDataSize(ids[i],&c,0,NULL,&n)||n<sizeof(AudioBufferList))continue;
        AudioBufferList *b=malloc(n);if(!b)continue;
        unsigned channels=0;
        if(!AudioObjectGetPropertyData(ids[i],&c,0,NULL,&n,b))for(unsigned j=0;j<b->mNumberBuffers;j++)channels+=b->mBuffers[j].mNumberChannels;
        free(b);if(!channels)continue;
        Device *d=&out[count++];memset(d,0,sizeof(*d));d->id=ids[i];d->channels=channels;
        CFStringRef name=NULL;n=sizeof(name);
        if(!get(ids[i],kAudioObjectPropertyName,kAudioObjectPropertyScopeGlobal,&name,&n)&&name){CFStringGetCString(name,d->name,sizeof(d->name),kCFStringEncodingUTF8);CFRelease(name);}
        n=sizeof(d->rate);get(ids[i],kAudioDevicePropertyNominalSampleRate,kAudioObjectPropertyScopeGlobal,&d->rate,&n);
        d->buffer_frames=uintprop(ids[i],kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal);
        d->latency_frames=uintprop(ids[i],kAudioDevicePropertyLatency,kAudioDevicePropertyScopeOutput);
        d->safety_frames=uintprop(ids[i],kAudioDevicePropertySafetyOffset,kAudioDevicePropertyScopeOutput);
    }
    free(ids);return count;
}
static OSStatus callback(void *ctx,AudioUnitRenderActionFlags *flags,const AudioTimeStamp *time,UInt32 bus,UInt32 frames,AudioBufferList *data) {
    (void)flags;(void)time;(void)bus;
    Output *o=ctx;
    if(data->mNumberBuffers==1&&data->mBuffers[0].mNumberChannels==2&&data->mBuffers[0].mData&&data->mBuffers[0].mDataByteSize>=frames*8) {
        double lead_ms=-1.;
        if(time->mFlags&kAudioTimeStampHostTimeValid) {
            UInt64 target=AudioConvertHostTimeToNanos(time->mHostTime);
            UInt64 now=AudioConvertHostTimeToNanos(AudioGetCurrentHostTime());
            lead_ms=((double)target-(double)now)/1e6;
        }
        o->render(o->context,data->mBuffers[0].mData,frames,lead_ms);
    } else {
        for(unsigned i=0;i<data->mNumberBuffers;i++)if(data->mBuffers[i].mData)memset(data->mBuffers[i].mData,0,data->mBuffers[i].mDataByteSize);
    }
    return noErr;
}
void lan_stop(Output *o) {
    if(!o)return;
    if(o->unit){AudioOutputUnitStop(o->unit);AudioUnitUninitialize(o->unit);AudioComponentInstanceDispose(o->unit);}
    // Restore only if nobody changed it after this receiver's explicit request.
    if(o->requested_buffer&&uintprop(o->device,kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal)==o->requested_buffer)
        set(o->device,kAudioDevicePropertyBufferFrameSize,&o->original_buffer,sizeof(UInt32));
    free(o);
}
Output *lan_start(Device *device,unsigned first_channel,unsigned buffer_frames,Render render,void *context,int *error) {
    Output *o=calloc(1,sizeof(*o));if(!o){*error=-1;return NULL;}
    o->device=device->id;o->render=render;o->context=context;o->original_buffer=device->buffer_frames;
    OSStatus err=0;
    #define CHECK(call) do {err=(call);if(err)goto fail;} while(0)
    if(first_channel+1>=device->channels){err=-50;goto fail;}
    if(buffer_frames){CHECK(set(device->id,kAudioDevicePropertyBufferFrameSize,&buffer_frames,sizeof(buffer_frames)));o->requested_buffer=buffer_frames;}
    AudioComponentDescription desc={kAudioUnitType_Output,kAudioUnitSubType_HALOutput,kAudioUnitManufacturer_Apple,0,0};
    AudioComponent component=AudioComponentFindNext(NULL,&desc);
    if(!component){err=-1;goto fail;}
    CHECK(AudioComponentInstanceNew(component,&o->unit));
    CHECK(AudioUnitSetProperty(o->unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&device->id,sizeof(device->id)));
    AudioStreamBasicDescription format={0};format.mSampleRate=device->rate;format.mFormatID=kAudioFormatLinearPCM;
    format.mFormatFlags=kAudioFormatFlagIsFloat|kAudioFormatFlagIsPacked;
    format.mBytesPerPacket=8;format.mFramesPerPacket=1;format.mBytesPerFrame=8;format.mChannelsPerFrame=2;format.mBitsPerChannel=32;
    CHECK(AudioUnitSetProperty(o->unit,kAudioUnitProperty_StreamFormat,kAudioUnitScope_Input,0,&format,sizeof(format)));
    SInt32 *map=malloc(device->channels*sizeof(*map));if(!map){err=-1;goto fail;}
    for(unsigned i=0;i<device->channels;i++)map[i]=-1;
    map[first_channel]=0;map[first_channel+1]=1;
    err=AudioUnitSetProperty(o->unit,kAudioOutputUnitProperty_ChannelMap,kAudioUnitScope_Input,0,map,device->channels*sizeof(*map));free(map);if(err)goto fail;
    AURenderCallbackStruct cb={callback,o};
    CHECK(AudioUnitSetProperty(o->unit,kAudioUnitProperty_SetRenderCallback,kAudioUnitScope_Input,0,&cb,sizeof(cb)));
    CHECK(AudioUnitInitialize(o->unit));
    device->buffer_frames=uintprop(device->id,kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal);
    CHECK(AudioOutputUnitStart(o->unit));
    *error=0;return o;
fail: *error=err;lan_stop(o);return NULL;
}

#include <sys/socket.h>
#include <sys/time.h>
#include <netinet/in.h>
#include <net/if_dl.h>
#include <unistd.h>
#include <errno.h>
typedef struct { unsigned char address[16]; unsigned short port; unsigned char ipv6; double kernel_queue_ms; unsigned interface_index; } Receipt;
int lan_prepare_socket(int fd) {
    int yes=1;
    if(setsockopt(fd,SOL_SOCKET,SO_TIMESTAMP,&yes,sizeof(yes)))return errno;
    // Optional ingress-interface metadata; IPv4 is the current LAN transport.
    setsockopt(fd,IPPROTO_IP,IP_RECVIF,&yes,sizeof(yes));
    return 0;
}
ssize_t lan_receive(int fd,void *bytes,size_t capacity,Receipt *receipt) {
    struct sockaddr_storage address={0};
    struct iovec iov={bytes,capacity};
    union {struct cmsghdr align;unsigned char bytes[512];} control;
    struct msghdr msg={0};msg.msg_name=&address;msg.msg_namelen=sizeof(address);msg.msg_iov=&iov;msg.msg_iovlen=1;msg.msg_control=control.bytes;msg.msg_controllen=sizeof(control);
    ssize_t count=recvmsg(fd,&msg,0);if(count<0)return count;
    memset(receipt,0,sizeof(*receipt));receipt->kernel_queue_ms=-1.;
    if(address.ss_family==AF_INET){struct sockaddr_in *a=(void*)&address;memcpy(receipt->address,&a->sin_addr,4);receipt->port=ntohs(a->sin_port);}
    else if(address.ss_family==AF_INET6){struct sockaddr_in6 *a=(void*)&address;memcpy(receipt->address,&a->sin6_addr,16);receipt->port=ntohs(a->sin6_port);receipt->ipv6=1;}
    else {errno=EAFNOSUPPORT;return -1;}
    struct timeval now;gettimeofday(&now,NULL);
    for(struct cmsghdr *c=CMSG_FIRSTHDR(&msg);c;c=CMSG_NXTHDR(&msg,c)) {
        if(c->cmsg_level==SOL_SOCKET&&c->cmsg_type==SCM_TIMESTAMP&&c->cmsg_len>=CMSG_LEN(sizeof(struct timeval))) {
            struct timeval stamp;memcpy(&stamp,CMSG_DATA(c),sizeof(stamp));
            receipt->kernel_queue_ms=(now.tv_sec-stamp.tv_sec)*1000.+(now.tv_usec-stamp.tv_usec)/1000.;
        }
        if(c->cmsg_level==IPPROTO_IP&&c->cmsg_type==IP_RECVIF&&c->cmsg_len>=CMSG_LEN(sizeof(struct sockaddr_dl))) {
            struct sockaddr_dl link;memcpy(&link,CMSG_DATA(c),sizeof(link));receipt->interface_index=link.sdl_index;
        }
    }
    return count;
}
