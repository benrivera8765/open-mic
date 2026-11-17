/*
 * Built-in WebView2 loader for the Windows GNU toolchain.
 *
 * Copyright (C) the Jukeoke contributors. SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Why this file exists
 * --------------------
 * Tauri's WebView2 bindings (crate webview2-com-sys) declare the five functions of
 * Microsoft's WebView2 loader. With the MSVC toolchain they link Microsoft's static
 * loader; with the GNU toolchain (the only one on our build PC, DECISIONS D-10) they
 * import them from WebView2Loader.dll, a separate Microsoft DLL that would have to
 * sit next to Jukeoke.exe on the USB stick.
 *
 * This file implements the same five functions, and is linked into the executable
 * as a whole archive (see build.rs), so the import is never needed and
 * Jukeoke.exe depends only on DLLs that ship with Windows 11. The approach (find the
 * installed WebView2 Runtime through the EdgeUpdate registry keys, load its
 * EmbeddedBrowserWebView.dll and call CreateWebViewEnvironmentWithOptionsInternal)
 * is the one used by the MIT-licensed webview/webview project and by go-webview2;
 * this is an independent implementation written for Jukeoke.
 *
 * Safety net: if no runtime is found this way (or the runtime lacks the expected
 * entry point), the functions delegate to Microsoft's WebView2Loader.dll, but only
 * when that DLL sits in the executable's own folder (never from the DLL search
 * path). Nothing ships it by default.
 *
 * Supported: Evergreen runtime (stable, then beta/dev/canary, per machine and per
 * user), a fixed-version runtime folder (parameter or WEBVIEW2_BROWSER_EXECUTABLE_FOLDER),
 * WEBVIEW2_USER_DATA_FOLDER and WEBVIEW2_RELEASE_CHANNEL_PREFERENCE=1.
 * Not supported (Jukeoke does not use them): the WebView2 group-policy overrides
 * and the packaged (MSIX) runtime.
 */

#define WIN32_LEAN_AND_MEAN
#ifndef UNICODE
#define UNICODE
#endif
#include <windows.h>
#include <objbase.h>
#include <stdbool.h>
#include <stdlib.h>
#include <wchar.h>

#if defined(__x86_64__) || defined(_M_X64)
#define JK_WV2_ARCH L"x64"
#elif defined(__aarch64__) || defined(_M_ARM64)
#define JK_WV2_ARCH L"arm64"
#elif defined(__i386__) || defined(_M_IX86)
#define JK_WV2_ARCH L"x86"
#else
#error "unsupported architecture for the WebView2 loader"
#endif

/* Oldest runtime the WebView2 SDK accepts (first general-availability release). */
static const unsigned MIN_VERSION[4] = {86, 0, 616, 0};

/* Release channels in default search order (stable first). */
static const struct {
    const wchar_t *guid;
    const wchar_t *suffix; /* appended to the version string, like the official loader */
} CHANNELS[] = {
    {L"{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}", L""},
    {L"{2CD8A007-E189-409D-A2C8-9AF4EF3C72AA}", L" beta"},
    {L"{0D50BFEC-CD6A-4F9A-964C-C7416E3ACB10}", L" dev"},
    {L"{65C35B14-6C1D-4122-AC46-7148CC9D6497}", L" canary"},
};
#define CHANNEL_COUNT (sizeof(CHANNELS) / sizeof(CHANNELS[0]))

/* Values passed to the runtime: an installed (Evergreen) runtime or a fixed one. */
enum runtime_kind { RUNTIME_INSTALLED = 0, RUNTIME_FIXED = 1 };

typedef HRESULT(STDMETHODCALLTYPE *create_internal_fn)(bool check_running_instance, int runtime_kind,
                                                       PCWSTR user_data_folder, IUnknown *options,
                                                       IUnknown *completed_handler);

#define PATH_CHARS 1024

typedef struct {
    wchar_t dll_path[PATH_CHARS]; /* ...\EBWebView\<arch>\EmbeddedBrowserWebView.dll */
    wchar_t version[64];          /* "157.0.4341.46" (+ channel suffix) */
    int kind;                     /* enum runtime_kind */
} found_runtime;

/* Diagnostics for the app (read by src/webview2_loader.rs). */
static wchar_t g_last_dll[PATH_CHARS];
static const char *g_last_route = "not used yet";

/* ---------- small helpers ---------- */

static bool file_exists(const wchar_t *path) {
    DWORD attr = GetFileAttributesW(path);
    return attr != INVALID_FILE_ATTRIBUTES && !(attr & FILE_ATTRIBUTE_DIRECTORY);
}

/* Parses "a.b.c.d" (anything after the fourth number is ignored). */
static bool parse_version(const wchar_t *s, unsigned out[4]) {
    for (int i = 0; i < 4; i++) {
        if (*s < L'0' || *s > L'9') return false;
        wchar_t *end = NULL;
        unsigned long v = wcstoul(s, &end, 10);
        out[i] = (unsigned)v;
        s = end;
        if (i < 3) {
            if (*s != L'.') return false;
            s++;
        }
    }
    return true;
}

static int compare_parts(const unsigned a[4], const unsigned b[4]) {
    for (int i = 0; i < 4; i++) {
        if (a[i] != b[i]) return a[i] < b[i] ? -1 : 1;
    }
    return 0;
}

/* Reads a REG_SZ value from the 32-bit registry view (where EdgeUpdate lives). */
static bool read_reg_string(HKEY root, const wchar_t *subkey, const wchar_t *name, wchar_t *out, DWORD out_chars) {
    HKEY key;
    if (RegOpenKeyExW(root, subkey, 0, KEY_READ | KEY_WOW64_32KEY, &key) != ERROR_SUCCESS) return false;
    DWORD bytes = out_chars * sizeof(wchar_t);
    LSTATUS status = RegGetValueW(key, NULL, name, RRF_RT_REG_SZ, NULL, out, &bytes);
    RegCloseKey(key);
    return status == ERROR_SUCCESS && out[0] != 0;
}

static bool read_env(const wchar_t *name, wchar_t *out, DWORD out_chars) {
    DWORD n = GetEnvironmentVariableW(name, out, out_chars);
    return n > 0 && n < out_chars;
}

/* Last path component, e.g. the version folder name. */
static const wchar_t *last_component(const wchar_t *path) {
    const wchar_t *last = path;
    for (const wchar_t *p = path; *p; p++) {
        if ((*p == L'\\' || *p == L'/') && p[1] != 0) last = p + 1;
    }
    return last;
}

static bool join_dll_path(const wchar_t *runtime_folder, wchar_t *out) {
    size_t len = wcslen(runtime_folder);
    while (len > 0 && (runtime_folder[len - 1] == L'\\' || runtime_folder[len - 1] == L'/')) len--;
    /* %ls (not %s): a wide string under both MSVCRT and C99 printf semantics. */
    int n = _snwprintf(out, PATH_CHARS, L"%.*ls\\EBWebView\\" JK_WV2_ARCH L"\\EmbeddedBrowserWebView.dll", (int)len,
                       runtime_folder);
    return n > 0 && n < PATH_CHARS;
}

/* Version of a fixed-version runtime, from the DLL's version resource. */
static bool file_version(const wchar_t *path, wchar_t *out, size_t out_chars) {
    DWORD ignored = 0;
    DWORD size = GetFileVersionInfoSizeW(path, &ignored);
    if (size == 0) return false;
    void *data = malloc(size);
    if (!data) return false;
    bool ok = false;
    VS_FIXEDFILEINFO *info = NULL;
    UINT info_len = 0;
    if (GetFileVersionInfoW(path, 0, size, data) && VerQueryValueW(data, L"\\", (void **)&info, &info_len) && info) {
        _snwprintf(out, out_chars, L"%u.%u.%u.%u", HIWORD(info->dwFileVersionMS), LOWORD(info->dwFileVersionMS),
                   HIWORD(info->dwFileVersionLS), LOWORD(info->dwFileVersionLS));
        out[out_chars - 1] = 0;
        ok = true;
    }
    free(data);
    return ok;
}

/* ---------- finding the runtime ---------- */

static HRESULT find_fixed(const wchar_t *folder, found_runtime *found) {
    if (!join_dll_path(folder, found->dll_path) || !file_exists(found->dll_path))
        return HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND);
    if (!file_version(found->dll_path, found->version, sizeof(found->version) / sizeof(wchar_t)))
        wcscpy(found->version, L"0.0.0.0");
    found->kind = RUNTIME_FIXED;
    return S_OK;
}

static bool try_channel(size_t channel, found_runtime *found) {
    static const HKEY roots[] = {HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER};
    wchar_t subkey[256];
    _snwprintf(subkey, 256, L"Software\\Microsoft\\EdgeUpdate\\ClientState\\%ls", CHANNELS[channel].guid);
    subkey[255] = 0;
    for (size_t r = 0; r < sizeof(roots) / sizeof(roots[0]); r++) {
        wchar_t folder[PATH_CHARS];
        if (!read_reg_string(roots[r], subkey, L"EBWebView", folder, PATH_CHARS)) continue;
        unsigned parts[4];
        const wchar_t *version = last_component(folder);
        if (!parse_version(version, parts) || compare_parts(parts, MIN_VERSION) < 0) continue;
        if (!join_dll_path(folder, found->dll_path) || !file_exists(found->dll_path)) continue;
        _snwprintf(found->version, sizeof(found->version) / sizeof(wchar_t), L"%ls%ls", version,
                   CHANNELS[channel].suffix);
        found->version[sizeof(found->version) / sizeof(wchar_t) - 1] = 0;
        found->kind = RUNTIME_INSTALLED;
        return true;
    }
    return false;
}

static HRESULT find_runtime(PCWSTR browser_folder, found_runtime *found) {
    memset(found, 0, sizeof(*found));
    if (browser_folder && browser_folder[0]) return find_fixed(browser_folder, found);

    wchar_t env[PATH_CHARS];
    if (read_env(L"WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", env, PATH_CHARS)) return find_fixed(env, found);

    bool reverse = read_env(L"WEBVIEW2_RELEASE_CHANNEL_PREFERENCE", env, PATH_CHARS) && wcscmp(env, L"1") == 0;
    for (size_t i = 0; i < CHANNEL_COUNT; i++) {
        size_t channel = reverse ? CHANNEL_COUNT - 1 - i : i;
        if (try_channel(channel, found)) return S_OK;
    }
    return HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND);
}

/* Microsoft's loader, only from the executable's own folder (safety net). */
static HMODULE official_loader(void) {
    static HMODULE module;
    static bool tried;
    if (tried) return module;
    tried = true;
    wchar_t path[PATH_CHARS];
    DWORD n = GetModuleFileNameW(NULL, path, PATH_CHARS);
    if (n == 0 || n >= PATH_CHARS) return NULL;
    wchar_t *slash = wcsrchr(path, L'\\');
    if (!slash) return NULL;
    slash[1] = 0;
    if (wcslen(path) + 20 >= PATH_CHARS) return NULL;
    wcscat(path, L"WebView2Loader.dll");
    if (!file_exists(path)) return NULL;
    module = LoadLibraryExW(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
    return module;
}

/* ---------- the five loader functions ---------- */

HRESULT STDAPICALLTYPE CreateCoreWebView2EnvironmentWithOptions(PCWSTR browser_folder, PCWSTR user_data_folder,
                                                                IUnknown *options, IUnknown *completed_handler) {
    if (!completed_handler) return E_POINTER;

    found_runtime found;
    HRESULT hr = find_runtime(browser_folder, &found);
    if (SUCCEEDED(hr)) {
        /* Deliberately never freed: the runtime stays in use for the life of the app. */
        HMODULE client = LoadLibraryExW(found.dll_path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
        create_internal_fn create =
            client ? (create_internal_fn)(void *)GetProcAddress(client, "CreateWebViewEnvironmentWithOptionsInternal")
                   : NULL;
        if (create) {
            wchar_t env_udf[PATH_CHARS];
            PCWSTR udf = read_env(L"WEBVIEW2_USER_DATA_FOLDER", env_udf, PATH_CHARS) ? env_udf : user_data_folder;
            wcsncpy(g_last_dll, found.dll_path, PATH_CHARS - 1);
            g_last_route = "built-in";
            return create(true, found.kind, udf, options, completed_handler);
        }
        hr = client ? HRESULT_FROM_WIN32(ERROR_PROC_NOT_FOUND) : HRESULT_FROM_WIN32(GetLastError());
    }

    HMODULE official = official_loader();
    if (official) {
        typedef HRESULT(STDAPICALLTYPE * fn_t)(PCWSTR, PCWSTR, IUnknown *, IUnknown *);
        fn_t fn = (fn_t)(void *)GetProcAddress(official, "CreateCoreWebView2EnvironmentWithOptions");
        if (fn) {
            g_last_route = "WebView2Loader.dll next to the exe";
            return fn(browser_folder, user_data_folder, options, completed_handler);
        }
    }
    g_last_route = "no WebView2 runtime found";
    return hr;
}

HRESULT STDAPICALLTYPE CreateCoreWebView2Environment(IUnknown *completed_handler) {
    return CreateCoreWebView2EnvironmentWithOptions(NULL, NULL, NULL, completed_handler);
}

HRESULT STDAPICALLTYPE GetAvailableCoreWebView2BrowserVersionString(PCWSTR browser_folder, LPWSTR *version_info) {
    if (!version_info) return E_POINTER;
    *version_info = NULL;
    found_runtime found;
    HRESULT hr = find_runtime(browser_folder, &found);
    if (FAILED(hr)) {
        HMODULE official = official_loader();
        if (official) {
            typedef HRESULT(STDAPICALLTYPE * fn_t)(PCWSTR, LPWSTR *);
            fn_t fn = (fn_t)(void *)GetProcAddress(official, "GetAvailableCoreWebView2BrowserVersionString");
            if (fn) return fn(browser_folder, version_info);
        }
        return hr;
    }
    size_t bytes = (wcslen(found.version) + 1) * sizeof(wchar_t);
    LPWSTR copy = (LPWSTR)CoTaskMemAlloc(bytes); /* the caller frees it with CoTaskMemFree */
    if (!copy) return E_OUTOFMEMORY;
    memcpy(copy, found.version, bytes);
    *version_info = copy;
    return S_OK;
}

HRESULT STDAPICALLTYPE GetAvailableCoreWebView2BrowserVersionStringWithOptions(PCWSTR browser_folder,
                                                                               IUnknown *options,
                                                                               LPWSTR *version_info) {
    (void)options; /* only carries channel-search options, which Jukeoke never sets */
    return GetAvailableCoreWebView2BrowserVersionString(browser_folder, version_info);
}

HRESULT STDAPICALLTYPE CompareBrowserVersions(PCWSTR version1, PCWSTR version2, int *result) {
    if (!version1 || !version2 || !result) return E_POINTER;
    unsigned a[4], b[4];
    if (!parse_version(version1, a) || !parse_version(version2, b)) return E_INVALIDARG;
    *result = compare_parts(a, b);
    return S_OK;
}

/*
 * webview2-com-sys declares these functions as imports from a DLL, so the compiled
 * Rust code calls them through import-table pointers named __imp_<function>. These
 * pointers stand in for the import table: they hold the addresses of the
 * functions above, so no WebView2Loader.dll import is ever created.
 */
#define JK_IMPORT_POINTER(fn) void *__imp_##fn = (void *)(fn)
JK_IMPORT_POINTER(CreateCoreWebView2EnvironmentWithOptions);
JK_IMPORT_POINTER(CreateCoreWebView2Environment);
JK_IMPORT_POINTER(GetAvailableCoreWebView2BrowserVersionString);
JK_IMPORT_POINTER(GetAvailableCoreWebView2BrowserVersionStringWithOptions);
JK_IMPORT_POINTER(CompareBrowserVersions);

/* ---------- diagnostics for the Rust side ---------- */

/* Which route the last environment creation took (static string, never freed). */
const char *jukeoke_webview2_loader_route(void) { return g_last_route; }

/* Full path of the EmbeddedBrowserWebView.dll used last ("" if none). */
const wchar_t *jukeoke_webview2_loader_dll(void) { return g_last_dll; }
