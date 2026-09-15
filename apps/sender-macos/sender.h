#pragma once
#include <stdint.h>
typedef struct { uint32_t id, channels, buffer_frames; double rate; char name[256]; char uid[256]; } SenderDevice;
typedef struct Sender Sender;
typedef struct { uint64_t packets, frames, send_errors, capture_errors; float peak; int last_error; } SenderStats;
int sender_devices(SenderDevice *out, int capacity);
Sender *sender_start(uint32_t device, uint32_t channel, const char *host, uint16_t port, const char *interface_ip, int *error);
void sender_stop(Sender *sender);
void sender_stats(Sender *sender, SenderStats *stats);
int sender_test(const char *host, uint16_t port);
