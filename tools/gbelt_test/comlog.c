#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#include <psapi.h>
#include <stdio.h>
#include <string.h>
#include <stdarg.h>

#pragma comment(lib, "psapi.lib")

static HANDLE g_log = INVALID_HANDLE_VALUE;
static HANDLE g_com_h[64];
static wchar_t g_com_n[64][96];
static volatile LONG g_ncom;

static HMODULE g_seen[512];
static int g_nseen;

typedef HANDLE (WINAPI *pCreateFileW)(LPCWSTR, DWORD, DWORD, LPSECURITY_ATTRIBUTES, DWORD, DWORD, HANDLE);
typedef HANDLE (WINAPI *pCreateFileA)(LPCSTR, DWORD, DWORD, LPSECURITY_ATTRIBUTES, DWORD, DWORD, HANDLE);
typedef BOOL (WINAPI *pWriteFile)(HANDLE, LPCVOID, DWORD, LPDWORD, LPOVERLAPPED);
typedef BOOL (WINAPI *pReadFile)(HANDLE, LPVOID, DWORD, LPDWORD, LPOVERLAPPED);
typedef BOOL (WINAPI *pSetCommState)(HANDLE, LPDCB);
typedef BOOL (WINAPI *pEscapeCommFunction)(HANDLE, DWORD);

static pCreateFileW real_CreateFileW, real_CreateFileW_k32;
static pCreateFileA real_CreateFileA, real_CreateFileA_k32;
static pWriteFile real_WriteFile, real_WriteFile_k32;
static pReadFile real_ReadFile, real_ReadFile_k32;
static pSetCommState real_SetCommState, real_SetCommState_k32;
static pEscapeCommFunction real_EscapeCommFunction, real_EscapeCommFunction_k32;

typedef LONG (WINAPI *NtWriteFile_t)(HANDLE, HANDLE, PVOID, PVOID, PVOID, PVOID, ULONG, PVOID, PVOID);
static NtWriteFile_t pNtWriteFile;

static void *follow(void *p) {
    BYTE *b = (BYTE *)p;
    int hops;
    if (!b) return p;
    for (hops = 0; hops < 4; hops++) {
        if (b[0] == 0xE9) {
            b = b + 5 + *(int *)(b + 1);
            continue;
        }
        if (b[0] == 0xFF && b[1] == 0x25) {
            b = *(BYTE **)(b + 6 + *(int *)(b + 2));
            continue;
        }
        break;
    }
    return b;
}

static void log_raw(const char *s, int n) {
    unsigned char iosb[16];
    if (g_log == INVALID_HANDLE_VALUE || n <= 0) return;
    if (!pNtWriteFile)
        pNtWriteFile = (NtWriteFile_t)GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtWriteFile");
    if (pNtWriteFile) {
        memset(iosb, 0, sizeof(iosb));
        pNtWriteFile(g_log, NULL, NULL, NULL, iosb, (PVOID)s, (ULONG)n, NULL, NULL);
    }
}

static void slog(const char *fmt, ...) {
    char buf[2048];
    va_list ap;
    int n;
    va_start(ap, fmt);
    n = _vsnprintf(buf, sizeof(buf) - 1, fmt, ap);
    va_end(ap);
    if (n > 0) log_raw(buf, n);
}

static int interesting_name(const wchar_t *n) {
    wchar_t u[256];
    int i;
    if (!n) return 0;
    for (i = 0; i < 255 && n[i]; i++) {
        wchar_t c = n[i];
        if (c >= L'a' && c <= L'z') c = (wchar_t)(c - 32);
        u[i] = c;
    }
    u[i] = 0;
    if (wcsstr(u, L"COMLOG") || wcsstr(u, L"COM.LOG") || wcsstr(u, L"IO.LOG")) return 0;
    if (wcsstr(u, L"COM3") || wcsstr(u, L"COM11") || wcsstr(u, L"COM14")) return 1;
    if (wcsstr(u, L"VCP") || wcsstr(u, L"SERIAL") || wcsstr(u, L"FTDI") ||
        wcsstr(u, L"FTSERT") || wcsstr(u, L"ACCUMOTION")) return 1;
    return 0;
}

static const wchar_t *name_for(HANDLE h) {
    int i, n = (int)g_ncom;
    for (i = 0; i < n && i < 64; i++)
        if (g_com_h[i] == h) return g_com_n[i];
    return NULL;
}

static void remember(HANDLE h, const wchar_t *n) {
    LONG i;
    if (h == INVALID_HANDLE_VALUE || h == NULL || !n) return;
    if (name_for(h)) return;
    i = InterlockedIncrement(&g_ncom) - 1;
    if (i < 0 || i >= 64) return;
    g_com_h[i] = h;
    wcsncpy(g_com_n[i], n, 95);
    g_com_n[i][95] = 0;
}

static int query_name(HANDLE h, wchar_t *out, int cap) {
    typedef LONG (WINAPI *QO)(HANDLE, ULONG, PVOID, ULONG, PULONG);
    static QO q;
    BYTE buf[1024];
    ULONG len = 0;
    USHORT slen;
    wchar_t *s;
    int n, i;
    if (!q) q = (QO)GetProcAddress(GetModuleHandleW(L"ntdll.dll"), "NtQueryObject");
    if (!q) return 0;
    if (q(h, 1, buf, sizeof(buf), &len) < 0) return 0;
    slen = *(USHORT *)buf;
    s = *(wchar_t **)(buf + 8);
    if (!s || !slen) return 0;
    n = slen / 2;
    if (n >= cap) n = cap - 1;
    for (i = 0; i < n; i++) out[i] = s[i];
    out[n] = 0;
    return 1;
}

static int is_com(HANDLE h) {
    wchar_t name[256];
    const wchar_t *known = name_for(h);
    if (known) return 1;
    if (!query_name(h, name, 256)) return 0;
    if (!interesting_name(name)) return 0;
    remember(h, name);
    return 1;
}

static void hexdump(const char *tag, HANDLE h, const BYTE *buf, DWORD n) {
    const wchar_t *nm = name_for(h);
    char line[1800];
    int o = 0;
    DWORD i, show;
    FILETIME ft;
    ULARGE_INTEGER ul;
    GetSystemTimeAsFileTime(&ft);
    ul.LowPart = ft.dwLowDateTime;
    ul.HighPart = ft.dwHighDateTime;
    o += _snprintf(line + o, sizeof(line) - o,
                   "%llu %s h=%p %ls n=%u :",
                   (unsigned long long)(ul.QuadPart / 10000), tag, h,
                   nm ? nm : L"?", (unsigned)n);
    show = n > 256 ? 256 : n;
    for (i = 0; i < show && o < (int)sizeof(line) - 8; i++)
        o += _snprintf(line + o, sizeof(line) - o, " %02X", buf[i]);
    if (n > show) o += _snprintf(line + o, sizeof(line) - o, " ...");
    o += _snprintf(line + o, sizeof(line) - o, " | ");
    for (i = 0; i < show && o < (int)sizeof(line) - 4; i++) {
        BYTE c = buf[i];
        line[o++] = (c >= 32 && c < 127) ? (char)c : '.';
    }
    line[o++] = '\n';
    log_raw(line, o);
}

static HANDLE WINAPI hook_CreateFileW(LPCWSTR path, DWORD acc, DWORD share,
    LPSECURITY_ATTRIBUTES sa, DWORD disp, DWORD flags, HANDLE tmpl) {
    HANDLE h = real_CreateFileW(path, acc, share, sa, disp, flags, tmpl);
    if (interesting_name(path)) {
        slog("CreateFileW %ls -> %p err=%u acc=%08X\n",
             path, h, (unsigned)GetLastError(), (unsigned)acc);
        if (h != INVALID_HANDLE_VALUE) remember(h, path);
    }
    return h;
}

static HANDLE WINAPI hook_CreateFileA(LPCSTR path, DWORD acc, DWORD share,
    LPSECURITY_ATTRIBUTES sa, DWORD disp, DWORD flags, HANDLE tmpl) {
    HANDLE h = real_CreateFileA(path, acc, share, sa, disp, flags, tmpl);
    wchar_t w[256];
    if (path) {
        MultiByteToWideChar(CP_ACP, 0, path, -1, w, 256);
        if (interesting_name(w)) {
            slog("CreateFileA %s -> %p err=%u\n", path, h, (unsigned)GetLastError());
            if (h != INVALID_HANDLE_VALUE) remember(h, w);
        }
    }
    return h;
}

static BOOL WINAPI hook_WriteFile(HANDLE h, LPCVOID buf, DWORD n, LPDWORD w, LPOVERLAPPED o) {
    if (buf && n && is_com(h)) hexdump("W", h, (const BYTE *)buf, n);
    return real_WriteFile(h, buf, n, w, o);
}

static BOOL WINAPI hook_ReadFile(HANDLE h, LPVOID buf, DWORD n, LPDWORD w, LPOVERLAPPED o) {
    BOOL ok = real_ReadFile(h, buf, n, w, o);
    DWORD got = 0;
    if (ok && is_com(h)) {
        if (w) got = *w;
        else if (o) got = n;
        if (got && buf) hexdump("R", h, (const BYTE *)buf, got);
    }
    return ok;
}

static BOOL WINAPI hook_SetCommState(HANDLE h, LPDCB d) {
    wchar_t name[256];
    if (is_com(h) && d) {
        if (!query_name(h, name, 256)) wcscpy(name, L"?");
        slog("SetCommState %ls baud=%u bits=%u parity=%u stop=%u dtr=%u rts=%u inX=%u outX=%u\n",
             name, (unsigned)d->BaudRate, (unsigned)d->ByteSize, (unsigned)d->Parity,
             (unsigned)d->StopBits, (unsigned)d->fDtrControl, (unsigned)d->fRtsControl,
             (unsigned)d->fInX, (unsigned)d->fOutX);
    }
    return real_SetCommState(h, d);
}

static BOOL WINAPI hook_EscapeCommFunction(HANDLE h, DWORD fn) {
    if (is_com(h)) slog("EscapeCommFunction %ls fn=%u\n", name_for(h) ? name_for(h) : L"?", (unsigned)fn);
    return real_EscapeCommFunction(h, fn);
}

static int already_seen(HMODULE m) {
    int i;
    for (i = 0; i < g_nseen; i++) if (g_seen[i] == m) return 1;
    if (g_nseen < 512) g_seen[g_nseen++] = m;
    return 0;
}

static void *hook_for(void *fn) {
    if (fn == (void *)real_CreateFileW || fn == (void *)real_CreateFileW_k32) return (void *)hook_CreateFileW;
    if (fn == (void *)real_CreateFileA || fn == (void *)real_CreateFileA_k32) return (void *)hook_CreateFileA;
    if (fn == (void *)real_WriteFile || fn == (void *)real_WriteFile_k32) return (void *)hook_WriteFile;
    if (fn == (void *)real_ReadFile || fn == (void *)real_ReadFile_k32) return (void *)hook_ReadFile;
    if (fn == (void *)real_SetCommState || fn == (void *)real_SetCommState_k32) return (void *)hook_SetCommState;
    if (fn == (void *)real_EscapeCommFunction || fn == (void *)real_EscapeCommFunction_k32)
        return (void *)hook_EscapeCommFunction;
    return NULL;
}

static void patch_iat(HMODULE mod) {
    BYTE *base = (BYTE *)mod;
    IMAGE_DOS_HEADER *dos;
    IMAGE_NT_HEADERS64 *nt;
    IMAGE_DATA_DIRECTORY *dir;
    IMAGE_IMPORT_DESCRIPTOR *imp;
    DWORD old;
    if (!base) return;
    __try {
        dos = (IMAGE_DOS_HEADER *)base;
        if (dos->e_magic != IMAGE_DOS_SIGNATURE) return;
        nt = (IMAGE_NT_HEADERS64 *)(base + dos->e_lfanew);
        if (nt->Signature != IMAGE_NT_SIGNATURE) return;
        if (nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC) return;
        dir = &nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
        if (!dir->VirtualAddress) return;
        imp = (IMAGE_IMPORT_DESCRIPTOR *)(base + dir->VirtualAddress);
        for (; imp->Name; imp++) {
            IMAGE_THUNK_DATA64 *th = (IMAGE_THUNK_DATA64 *)(base + imp->FirstThunk);
            for (; th->u1.Function; th++) {
                void *fn = (void *)(ULONG_PTR)th->u1.Function;
                void *hk = hook_for(fn);
                if (!hk) continue;
                if (!VirtualProtect(&th->u1.Function, sizeof(ULONGLONG), PAGE_READWRITE, &old)) continue;
                th->u1.Function = (ULONGLONG)(ULONG_PTR)hk;
                VirtualProtect(&th->u1.Function, sizeof(ULONGLONG), old, &old);
            }
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
    }
}

static void patch_all(void) {
    HMODULE mods[384];
    DWORD needed = 0;
    unsigned i, n;
    if (!EnumProcessModules(GetCurrentProcess(), mods, sizeof(mods), &needed)) return;
    n = needed / sizeof(HMODULE);
    if (n > 384) n = 384;
    for (i = 0; i < n; i++) {
        int first = !already_seen(mods[i]);
        patch_iat(mods[i]);
        if (first) {
            wchar_t name[MAX_PATH];
            name[0] = 0;
            GetModuleFileNameW(mods[i], name, MAX_PATH);
            slog("module %ls\n", name);
        }
    }
}

static DWORD WINAPI worker(LPVOID arg) {
    int i;
    (void)arg;
    slog("comlog worker pid=%u\n", (unsigned)GetCurrentProcessId());
    for (i = 0; i < 200; i++) {
        patch_all();
        Sleep(i < 40 ? 100 : 500);
    }
    for (;;) {
        patch_all();
        Sleep(1000);
    }
    return 0;
}

static void *gp(HMODULE m, const char *n) {
    void *p;
    if (!m) return NULL;
    p = GetProcAddress(m, n);
    return follow(p);
}

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID res) {
    HMODULE k32, kb;
    (void)res;
    if (reason != DLL_PROCESS_ATTACH) return TRUE;
    DisableThreadLibraryCalls(inst);
    CreateDirectoryW(L"C:\\Users\\playe\\com3sniff", NULL);
    g_log = CreateFileW(L"C:\\Users\\playe\\com3sniff\\com.log",
                        FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                        NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    slog("=== comlog IAT attach pid=%u ===\n", (unsigned)GetCurrentProcessId());
    k32 = GetModuleHandleW(L"kernel32.dll");
    kb = GetModuleHandleW(L"kernelbase.dll");
    real_CreateFileW_k32 = (pCreateFileW)gp(k32, "CreateFileW");
    real_CreateFileA_k32 = (pCreateFileA)gp(k32, "CreateFileA");
    real_WriteFile_k32 = (pWriteFile)gp(k32, "WriteFile");
    real_ReadFile_k32 = (pReadFile)gp(k32, "ReadFile");
    real_SetCommState_k32 = (pSetCommState)gp(k32, "SetCommState");
    real_EscapeCommFunction_k32 = (pEscapeCommFunction)gp(k32, "EscapeCommFunction");
    real_CreateFileW = (pCreateFileW)gp(kb, "CreateFileW");
    real_CreateFileA = (pCreateFileA)gp(kb, "CreateFileA");
    real_WriteFile = (pWriteFile)gp(kb, "WriteFile");
    real_ReadFile = (pReadFile)gp(kb, "ReadFile");
    real_SetCommState = (pSetCommState)gp(kb, "SetCommState");
    real_EscapeCommFunction = (pEscapeCommFunction)gp(kb, "EscapeCommFunction");
    if (!real_CreateFileW) real_CreateFileW = real_CreateFileW_k32;
    if (!real_CreateFileA) real_CreateFileA = real_CreateFileA_k32;
    if (!real_WriteFile) real_WriteFile = real_WriteFile_k32;
    if (!real_ReadFile) real_ReadFile = real_ReadFile_k32;
    if (!real_SetCommState) real_SetCommState = real_SetCommState_k32;
    if (!real_EscapeCommFunction) real_EscapeCommFunction = real_EscapeCommFunction_k32;
    slog("orig WriteFile kb=%p k32=%p\n", real_WriteFile, real_WriteFile_k32);
    CreateThread(NULL, 0, worker, NULL, 0, NULL);
    return TRUE;
}
