// Native macOS process sampler for the desktop resource benchmark.
// Usage: sampler interval_ms ui_pid gateway_pid [ui_pid gateway_pid ...]
#include <errno.h>
#include <libproc.h>
#include <limits.h>
#include <mach/mach_time.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#define PID_COALITION_INFO 20

struct coalition_info {
    uint64_t ids[2];
    uint64_t reserved[3];
};

struct sample {
    pid_t pid;
    pid_t ppid;
    char name[PROC_PIDPATHINFO_MAXSIZE];
    struct coalition_info coalition;
    struct rusage_info_v4 usage;
    struct timespec measured;
};

int main(int argc, char **argv) {
    if (argc < 4 || (argc - 2) % 2 != 0) {
        fprintf(stderr, "usage: %s interval_ms ui_pid gateway_pid [...]\n", argv[0]);
        return 2;
    }
    for (int i = 1; i < argc; i++) {
        char *end;
        errno = 0;
        long value = strtol(argv[i], &end, 10);
        if (errno || *argv[i] == '\0' || *end != '\0' || value < (i == 1 ? 0 : 1) || value > INT_MAX) {
            fprintf(stderr, "invalid numeric argument: %s\n", argv[i]);
            return 2;
        }
    }
    const int interval_ms = atoi(argv[1]);
    const int products = (argc - 2) / 2;
    mach_timebase_info_data_t timebase;
    if (mach_timebase_info(&timebase) != KERN_SUCCESS) return 1;
    setvbuf(stdout, NULL, _IOLBF, 0);
    do {
        int bytes = proc_listallpids(NULL, 0);
        if (bytes <= 0) return 1;
        int capacity = bytes + 256;
        pid_t *pids = calloc((size_t)capacity, sizeof(*pids));
        struct sample *samples = calloc((size_t)capacity, sizeof(*samples));
        if (!pids || !samples) return 1;
        int count = proc_listallpids(pids, capacity * (int)sizeof(*pids));
        int n = 0;
        for (int i = 0; i < count; i++) {
            struct proc_bsdinfo info;
            struct sample s = {0};
            s.pid = pids[i];
            if (s.pid <= 0 || proc_pidinfo(s.pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info)) != sizeof(info)) continue;
            if (info.pbi_uid != getuid()) continue;
            if (proc_pidinfo(s.pid, PID_COALITION_INFO, 0, &s.coalition, sizeof(s.coalition)) != sizeof(s.coalition)) continue;
            if (proc_pid_rusage(s.pid, RUSAGE_INFO_V4, (rusage_info_t *)&s.usage) != 0) continue;
            clock_gettime(CLOCK_MONOTONIC, &s.measured);
            s.ppid = info.pbi_ppid;
            proc_name(s.pid, s.name, sizeof(s.name));
            samples[n++] = s;
        }
        struct timespec now;
        clock_gettime(CLOCK_MONOTONIC, &now);
        printf("{\"monotonic\":%.9f,\"timebase\":[%u,%u],\"products\":[", now.tv_sec + now.tv_nsec / 1e9, timebase.numer, timebase.denom);
        for (int p = 0; p < products; p++) {
            pid_t ui = (pid_t)atoi(argv[2 + p * 2]);
            pid_t gateway = (pid_t)atoi(argv[3 + p * 2]);
            uint64_t coalition = 0;
            for (int i = 0; i < n; i++) if (samples[i].pid == ui) coalition = samples[i].coalition.ids[0];
            if (p) putchar(',');
            printf("{\"ui\":%d,\"gateway\":%d,\"coalition\":%llu,\"members\":[", ui, gateway, (unsigned long long)coalition);
            int first = 1;
            for (int i = 0; i < n; i++) {
                struct sample *s = &samples[i];
                if (!coalition || s->coalition.ids[0] != coalition) continue;
                if (!first) putchar(',');
                first = 0;
                printf("{\"pid\":%d,\"ppid\":%d,\"name\":\"", s->pid, s->ppid);
                for (const unsigned char *c = (const unsigned char *)s->name; *c; c++) {
                    if (*c == '"' || *c == '\\') printf("\\%c", *c);
                    else if (*c < 0x20) printf("\\u%04x", *c);
                    else putchar(*c);
                }
                printf("\",\"measuredMonotonic\":%.9f,\"coalition\":[%llu,%llu],\"rss\":%llu,\"footprint\":%llu,\"cpuTicks\":%llu}", s->measured.tv_sec + s->measured.tv_nsec / 1e9,
                    (unsigned long long)s->coalition.ids[0], (unsigned long long)s->coalition.ids[1],
                    (unsigned long long)s->usage.ri_resident_size, (unsigned long long)s->usage.ri_phys_footprint,
                    (unsigned long long)(s->usage.ri_user_time + s->usage.ri_system_time + s->usage.ri_child_user_time + s->usage.ri_child_system_time));
            }
            printf("]}");
        }
        printf("]}\n");
        free(samples);
        free(pids);
        if (interval_ms) {
            struct timespec delay = {interval_ms / 1000, (interval_ms % 1000) * 1000000L};
            while (nanosleep(&delay, &delay) && errno == EINTR) {}
        }
    } while (interval_ms);
    return 0;
}
