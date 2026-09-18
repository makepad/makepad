#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>
#include <shellapi.h>

#pragma comment(lib, "shell32.lib")
#pragma comment(lib, "advapi32.lib")

static int is_elevated(void) {
    HANDLE tok = NULL;
    TOKEN_ELEVATION el;
    DWORD n = 0;
    int ok = 0;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &tok)) return 0;
    if (GetTokenInformation(tok, TokenElevation, &el, sizeof(el), &n))
        ok = el.TokenIsElevated ? 1 : 0;
    CloseHandle(tok);
    return ok;
}

static int self_elevate(void) {
    wchar_t path[MAX_PATH];
    SHELLEXECUTEINFOW sei;
    GetModuleFileNameW(NULL, path, MAX_PATH);
    memset(&sei, 0, sizeof(sei));
    sei.cbSize = sizeof(sei);
    sei.lpVerb = L"runas";
    sei.lpFile = path;
    sei.nShow = SW_SHOWNORMAL;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS;
    printf("Commander requires elevation (UAC error 740).\n");
    printf("Accept the UAC prompt — logging continues in the new window.\n");
    fflush(stdout);
    if (!ShellExecuteExW(&sei)) {
        printf("UAC failed/cancelled %u\n", GetLastError());
        return 0;
    }
    if (sei.hProcess) CloseHandle(sei.hProcess);
    return 1;
}

static DWORD find_commander(void) {
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    PROCESSENTRY32W pe;
    DWORD pid = 0;
    if (snap == INVALID_HANDLE_VALUE) return 0;
    pe.dwSize = sizeof(pe);
    if (Process32FirstW(snap, &pe)) {
        do {
            if (_wcsicmp(pe.szExeFile, L"Commander4.exe") == 0) {
                pid = pe.th32ProcessID;
                break;
            }
        } while (Process32NextW(snap, &pe));
    }
    CloseHandle(snap);
    return pid;
}

static int inject(HANDLE proc, const wchar_t *dll) {
    size_t bytes = (wcslen(dll) + 1) * sizeof(wchar_t);
    void *remote = VirtualAllocEx(proc, NULL, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    HANDLE th;
    DWORD code = 0;
    if (!remote) {
        printf("VirtualAllocEx failed %u\n", GetLastError());
        return 0;
    }
    if (!WriteProcessMemory(proc, remote, dll, bytes, NULL)) {
        printf("WriteProcessMemory failed %u\n", GetLastError());
        return 0;
    }
    th = CreateRemoteThread(proc, NULL, 0,
        (LPTHREAD_START_ROUTINE)GetProcAddress(GetModuleHandleW(L"kernel32.dll"), "LoadLibraryW"),
        remote, 0, NULL);
    if (!th) {
        printf("CreateRemoteThread failed %u\n", GetLastError());
        return 0;
    }
    WaitForSingleObject(th, 15000);
    GetExitCodeThread(th, &code);
    CloseHandle(th);
    printf("LoadLibraryW -> %p\n", (void *)(ULONG_PTR)code);
    return code != 0;
}

static void tail_log(HANDLE proc) {
    HANDLE log;
    DWORD pos = 0;
    char buf[4096];
    DWORD n;
    log = CreateFileW(L"C:\\Users\\playe\\com3sniff\\com.log", GENERIC_READ,
                      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                      NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (log == INVALID_HANDLE_VALUE) {
        printf("cannot open com.log %u\n", GetLastError());
        WaitForSingleObject(proc, INFINITE);
        return;
    }
    SetFilePointer(log, 0, NULL, FILE_END);
    pos = SetFilePointer(log, 0, NULL, FILE_CURRENT);
    printf("--- live COM log (close Commander when done) ---\n");
    fflush(stdout);
    for (;;) {
        DWORD wr = WaitForSingleObject(proc, 200);
        SetFilePointer(log, pos, NULL, FILE_BEGIN);
        while (ReadFile(log, buf, sizeof(buf) - 1, &n, NULL) && n) {
            DWORD w;
            WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), buf, n, &w, NULL);
            pos += n;
        }
        if (wr == WAIT_OBJECT_0) break;
    }
    CloseHandle(log);
    printf("--- Commander exited ---\n");
}

int main(void) {
    STARTUPINFOW si;
    PROCESS_INFORMATION pi;
    wchar_t exe[] = L"C:\\Program Files (x86)\\SimXperience\\Sim Commander 4.5\\Commander4.exe";
    wchar_t dir[] = L"C:\\Program Files (x86)\\SimXperience\\Sim Commander 4.5";
    wchar_t dll[] = L"C:\\Users\\playe\\com3sniff\\comlog64.dll";
    wchar_t cmd[512];
    DWORD existing;
    HANDLE log;

    printf("G-Belt COM logger\n");
    printf("Starts Sim Commander with an IAT WriteFile hook (no INT3).\n");
    printf("Log: C:\\Users\\playe\\com3sniff\\com.log\n\n");

    if (!is_elevated()) {
        if (!self_elevate()) return 1;
        return 0;
    }
    printf("elevated pid=%u\n", GetCurrentProcessId());

    existing = find_commander();
    if (existing) {
        printf("Commander4.exe is already running (pid %u).\n", existing);
        printf("Close it first, then run this again so we can capture INITIALIZE.\n");
        return 2;
    }

    CreateDirectoryW(L"C:\\Users\\playe\\com3sniff", NULL);
    log = CreateFileW(L"C:\\Users\\playe\\com3sniff\\com.log", GENERIC_WRITE,
                      FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, CREATE_ALWAYS,
                      FILE_ATTRIBUTE_NORMAL, NULL);
    if (log != INVALID_HANDLE_VALUE) CloseHandle(log);

    memset(&si, 0, sizeof(si));
    si.cb = sizeof(si);
    _snwprintf(cmd, 500, L"\"%s\"", exe);
    if (!CreateProcessW(exe, cmd, NULL, NULL, FALSE, CREATE_SUSPENDED, NULL, dir, &si, &pi)) {
        DWORD err = GetLastError();
        printf("CreateProcess failed %u\n", err);
        if (err == 740)
            printf("Still unelevated. Right-click Log G-Belt COM and Run as administrator.\n");
        return 1;
    }
    printf("started pid=%u tid=%u (suspended)\n", pi.dwProcessId, pi.dwThreadId);
    if (!inject(pi.hProcess, dll)) {
        printf("inject failed, killing child\n");
        TerminateProcess(pi.hProcess, 1);
        return 1;
    }
    ResumeThread(pi.hThread);
    CloseHandle(pi.hThread);
    printf("resumed. In Commander: connect/initialize the AccuMotion / G-Belt.\n");
    fflush(stdout);
    tail_log(pi.hProcess);
    CloseHandle(pi.hProcess);
    return 0;
}
