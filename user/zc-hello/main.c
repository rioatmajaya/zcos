/* Minimal C proof task for ZC OS. */
#include <stddef.h>

static inline long zc_syscall1(long n, long a) {
    long r;
    __asm__ volatile("int $0x80" : "=a"(r) : "a"(n), "D"(a) : "rcx", "r11", "memory");
    return r;
}

static inline long zc_syscall2(long n, long a, long b) {
    long r;
    __asm__ volatile("int $0x80" : "=a"(r) : "a"(n), "D"(a), "S"(b) : "rcx", "r11", "memory");
    return r;
}

static void zc_log(const char *s) {
    long len = 0;
    while (s[len]) len++;
    zc_syscall2(6, (long)s, len); /* SYS_LOG_WRITE */
}

void _start(void) {
    zc_log("hello from C\n");
    zc_syscall1(5, 0); /* SYS_TASK_EXIT */
    for (;;) { }
}
