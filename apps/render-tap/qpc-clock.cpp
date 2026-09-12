#include <windows.h>
#include <cstdio>
int main(){
    LARGE_INTEGER frequency;QueryPerformanceFrequency(&frequency);
    char line[32];while(fgets(line,sizeof(line),stdin)){
        LARGE_INTEGER now;QueryPerformanceCounter(&now);
        printf("%.9f\n",double(now.QuadPart)*1000.0/double(frequency.QuadPart));fflush(stdout);
    }
}
