// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostics from the built-in WebView2 loader (native/webview2_loader.c), which
//! replaces Microsoft's WebView2Loader.dll on the Windows GNU target (see build.rs).

use serde::Serialize;

/// Which loader created the web view environment and which runtime DLL it used.
#[derive(Debug, Clone, Serialize)]
pub struct LoaderInfo {
    /// "built-in", "WebView2Loader.dll next to the exe", "no WebView2 runtime found",
    /// "not used yet", or "platform loader" where the C loader is not built.
    pub route: String,
    /// Full path of EmbeddedBrowserWebView.dll, if the built-in route was taken.
    pub runtime_dll: Option<String>,
}

#[cfg(all(windows, target_env = "gnu"))]
mod ffi {
    unsafe extern "C" {
        pub fn jukeoke_webview2_loader_route() -> *const std::ffi::c_char;
        pub fn jukeoke_webview2_loader_dll() -> *const u16;
    }
}

#[cfg(all(windows, target_env = "gnu"))]
pub fn info() -> LoaderInfo {
    // SAFETY: both functions return pointers to static, NUL-terminated strings owned
    // by the C file; they are only written while the web view environment is being
    // created, which has finished by the time diagnostics are read.
    unsafe {
        let route = std::ffi::CStr::from_ptr(ffi::jukeoke_webview2_loader_route())
            .to_string_lossy()
            .into_owned();
        let dll = ffi::jukeoke_webview2_loader_dll();
        let mut len = 0;
        while *dll.add(len) != 0 {
            len += 1;
        }
        let dll = String::from_utf16_lossy(std::slice::from_raw_parts(dll, len));
        LoaderInfo {
            route,
            runtime_dll: (!dll.is_empty()).then_some(dll),
        }
    }
}

#[cfg(not(all(windows, target_env = "gnu")))]
pub fn info() -> LoaderInfo {
    LoaderInfo {
        route: "platform loader".into(),
        runtime_dll: None,
    }
}

/// Exercises the C loader directly (Windows GNU builds only). Needs the WebView2
/// Runtime, which every Windows 11 PC and GitHub's Windows runners have.
#[cfg(all(test, windows, target_env = "gnu"))]
mod tests {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::core::{HRESULT, PCWSTR, PWSTR, w};

    unsafe extern "system" {
        fn GetAvailableCoreWebView2BrowserVersionString(
            browser_folder: PCWSTR,
            version: *mut PWSTR,
        ) -> HRESULT;
        fn CompareBrowserVersions(a: PCWSTR, b: PCWSTR, result: *mut i32) -> HRESULT;
    }

    #[test]
    fn finds_the_installed_runtime_version() {
        let mut version = PWSTR::null();
        // SAFETY: valid out-pointer; the string is freed with CoTaskMemFree as documented.
        let text = unsafe {
            let hr = GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version);
            assert!(hr.is_ok(), "no WebView2 Runtime found: {hr:?}");
            let text = version.to_string().unwrap();
            CoTaskMemFree(Some(version.0 as *const core::ffi::c_void));
            text
        };
        let parts: Vec<u32> = text
            .split(' ')
            .next()
            .unwrap()
            .split('.')
            .map(|p| p.parse().unwrap())
            .collect();
        assert_eq!(parts.len(), 4, "version {text:?}");
        assert!(
            parts[0] >= 86,
            "runtime {text:?} older than the WebView2 minimum"
        );
    }

    #[test]
    fn missing_fixed_runtime_folder_is_not_found() {
        let mut version = PWSTR::null();
        // SAFETY: valid pointers; nothing is allocated on failure.
        let hr = unsafe {
            GetAvailableCoreWebView2BrowserVersionString(w!(r"Z:\no\such\runtime"), &mut version)
        };
        assert_eq!(hr, HRESULT::from_win32(2)); // ERROR_FILE_NOT_FOUND
        assert!(version.is_null());
    }

    #[test]
    fn compares_versions_numerically() {
        let mut result = 0;
        // SAFETY: valid NUL-terminated strings and out-pointer.
        unsafe {
            assert!(CompareBrowserVersions(w!("100.0.1.2"), w!("99.9.9.9"), &mut result).is_ok());
            assert_eq!(result, 1);
            assert!(
                CompareBrowserVersions(w!("157.0.4341.46"), w!("157.0.4341.46 beta"), &mut result)
                    .is_ok()
            );
            assert_eq!(result, 0);
            assert!(CompareBrowserVersions(w!("86.0.616.0"), w!("157.0.0.0"), &mut result).is_ok());
            assert_eq!(result, -1);
            assert!(CompareBrowserVersions(w!("bad"), w!("1.2.3.4"), &mut result).is_err());
        }
    }
}
