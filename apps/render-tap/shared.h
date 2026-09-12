#pragma once
#include <windows.h>
#include <cstdint>
constexpr unsigned Slots=128, MaxBytes=32768;
struct Format {unsigned rate, channels, align, bits, tag;};
struct Packet {
    volatile LONG ready;
    unsigned frames, flags, bytes; Format format;
    uint64_t stream, entry, copied, released; long result;
    BYTE pcm[MaxBytes];
};
struct Shared {
    volatile LONG installed, finished, error, dropped, unknown, busy, calls;
    Packet packets[Slots];
};
struct Config {HANDLE mapping,event; uint64_t functions[5]; unsigned seconds;};
inline uint64_t tick(){LARGE_INTEGER n;QueryPerformanceCounter(&n);return n.QuadPart;}
