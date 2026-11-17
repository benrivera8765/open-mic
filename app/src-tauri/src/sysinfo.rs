// SPDX-License-Identifier: GPL-3.0-or-later
//! Small facts about the computer: OS version, memory, free space on a volume, local
//! time for file names. Direct OS calls (no extra crates): Windows API on Windows,
//! sysctl / statvfs / Mach on macOS.

use std::path::Path;

/// Memory figures in bytes. `None` where the OS did not say.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Memory {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    /// Resident set size (working set) of this process.
    pub process_rss_bytes: Option<u64>,
}

/// "Windows 11 26200" / "macOS 13.6.9" (docs/APP_API.md `AppInfo.os_version`).
pub fn os_version() -> String {
    imp::os_version()
}

pub fn memory() -> Memory {
    imp::memory()
}

/// (free bytes available to this user, total bytes) of the volume holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    imp::disk_space(path)
}

/// Local wall-clock time (year, month, day, hour, minute, second).
pub fn local_time() -> (u32, u32, u32, u32, u32, u32) {
    imp::local_time()
}

/// "2026-11-17 21-05-09": local time, safe in file names on every OS and exFAT.
pub fn local_stamp() -> String {
    let (y, mo, d, h, mi, s) = local_time();
    format!("{y:04}-{mo:02}-{d:02} {h:02}-{mi:02}-{s:02}")
}

#[cfg(windows)]
mod imp {
    use super::Memory;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::Wdk::System::SystemServices::RtlGetVersion;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::SystemInformation::{
        GetLocalTime, GlobalMemoryStatusEx, MEMORYSTATUSEX, OSVERSIONINFOW,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::core::PCWSTR;

    pub fn os_version() -> String {
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        // SAFETY: RtlGetVersion fills the struct whose size we set; it cannot fail for
        // a correctly sized OSVERSIONINFOW. Unlike GetVersionEx it never lies about the
        // version (no compatibility shims).
        let status = unsafe { RtlGetVersion(&mut info) };
        if status.is_err() {
            return "Windows (version unknown)".into();
        }
        // Windows 11 still reports major version 10; build 22000 is the first Windows 11.
        let name = if info.dwMajorVersion == 10 && info.dwBuildNumber >= 22000 {
            "11".to_string()
        } else if info.dwMajorVersion == 10 {
            "10".to_string()
        } else {
            format!("{}.{}", info.dwMajorVersion, info.dwMinorVersion)
        };
        format!("Windows {name} {}", info.dwBuildNumber)
    }

    pub fn memory() -> Memory {
        let mut m = Memory::default();
        let mut st = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: dwLength is set as the API requires.
        if unsafe { GlobalMemoryStatusEx(&mut st) }.is_ok() {
            m.total_bytes = Some(st.ullTotalPhys);
            m.available_bytes = Some(st.ullAvailPhys);
        }
        let mut pmc = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: the pseudo handle of the current process needs no closing; cb is set.
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) }.is_ok() {
            m.process_rss_bytes = Some(pmc.WorkingSetSize as u64);
        }
        m
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: NUL-terminated path; out-pointers are valid u64s.
        unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(wide.as_ptr()),
                Some(&mut free),
                Some(&mut total),
                None,
            )
        }
        .ok()?;
        Some((free, total))
    }

    pub fn local_time() -> (u32, u32, u32, u32, u32, u32) {
        // SAFETY: no arguments; returns a SYSTEMTIME by value.
        let t = unsafe { GetLocalTime() };
        (
            t.wYear as u32,
            t.wMonth as u32,
            t.wDay as u32,
            t.wHour as u32,
            t.wMinute as u32,
            t.wSecond as u32,
        )
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Memory;
    use std::ffi::CString;
    use std::path::Path;

    fn sysctl_string(name: &str) -> Option<String> {
        let cname = CString::new(name).ok()?;
        let mut len: libc::size_t = 0;
        // SAFETY: first call asks for the length only.
        if unsafe { libc::sysctlbyname(cname.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) } != 0 {
            return None;
        }
        let mut buf = vec![0u8; len];
        // SAFETY: buf has the length the first call reported.
        if unsafe { libc::sysctlbyname(cname.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } != 0 {
            return None;
        }
        buf.truncate(len);
        while buf.last() == Some(&0) {
            buf.pop();
        }
        String::from_utf8(buf).ok()
    }

    fn sysctl_u64(name: &str) -> Option<u64> {
        let cname = CString::new(name).ok()?;
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: v is a u64 and len says so.
        let r = unsafe {
            libc::sysctlbyname(cname.as_ptr(), (&mut v as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0)
        };
        (r == 0).then_some(v)
    }

    pub fn os_version() -> String {
        // kern.osproductversion exists since macOS 10.13.4.
        match sysctl_string("kern.osproductversion") {
            Some(v) => {
                let build = sysctl_string("kern.osversion").map(|b| format!(" ({b})")).unwrap_or_default();
                format!("macOS {v}{build}")
            }
            None => "macOS (version unknown)".into(),
        }
    }

    #[allow(deprecated)] // libc marks the Mach structs deprecated in favour of the mach2 crate
    pub fn memory() -> Memory {
        let mut m = Memory {
            total_bytes: sysctl_u64("hw.memsize"),
            ..Default::default()
        };
        // Available = free + inactive + purgeable + speculative pages (what Activity
        // Monitor calls memory that can be used without swapping, roughly).
        // SAFETY: host_statistics64 fills a vm_statistics64 of the count we pass.
        unsafe {
            let mut stats: libc::vm_statistics64 = std::mem::zeroed();
            let mut count = (std::mem::size_of::<libc::vm_statistics64>() / std::mem::size_of::<libc::integer_t>())
                as libc::mach_msg_type_number_t;
            let host = libc::mach_host_self();
            if libc::host_statistics64(host, libc::HOST_VM_INFO64, (&mut stats as *mut libc::vm_statistics64).cast(), &mut count) == 0 {
                let page = sysctl_u64("hw.pagesize").unwrap_or(16384);
                let pages = stats.free_count as u64 + stats.inactive_count as u64 + stats.purgeable_count as u64
                    + stats.speculative_count as u64;
                m.available_bytes = Some(pages * page);
            }
            let mut info: libc::mach_task_basic_info = std::mem::zeroed();
            let mut count = (std::mem::size_of::<libc::mach_task_basic_info>() / std::mem::size_of::<libc::natural_t>())
                as libc::mach_msg_type_number_t;
            if libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                (&mut info as *mut libc::mach_task_basic_info).cast(),
                &mut count,
            ) == 0
            {
                m.process_rss_bytes = Some(info.resident_size);
            }
        }
        m
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: valid C string and out struct.
        unsafe {
            let mut s: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut s) != 0 {
                return None;
            }
            let frsize = s.f_frsize as u64;
            Some((s.f_bavail as u64 * frsize, s.f_blocks as u64 * frsize))
        }
    }

    pub fn local_time() -> (u32, u32, u32, u32, u32, u32) {
        // SAFETY: time(NULL) and localtime_r with a valid out struct.
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            (
                (tm.tm_year + 1900) as u32,
                (tm.tm_mon + 1) as u32,
                tm.tm_mday as u32,
                tm.tm_hour as u32,
                tm.tm_min as u32,
                tm.tm_sec as u32,
            )
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::Memory;
    use std::path::Path;
    pub fn os_version() -> String {
        std::env::consts::OS.to_string()
    }
    pub fn memory() -> Memory {
        Memory::default()
    }
    pub fn disk_space(_path: &Path) -> Option<(u64, u64)> {
        None
    }
    pub fn local_time() -> (u32, u32, u32, u32, u32, u32) {
        (1970, 1, 1, 0, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_are_plausible() {
        let v = os_version();
        assert!(v.starts_with("Windows") || v.starts_with("macOS") || !cfg!(any(windows, target_os = "macos")), "{v}");
        if cfg!(any(windows, target_os = "macos")) {
            let m = memory();
            let total = m.total_bytes.unwrap();
            assert!(total > 1 << 30, "total {total}");
            assert!(m.available_bytes.unwrap() <= total);
            assert!(m.process_rss_bytes.unwrap() > 0);
            let (free, total) = disk_space(&std::env::temp_dir()).unwrap();
            assert!(free <= total && total > 0);
            let (y, mo, d, ..) = local_time();
            assert!(y >= 2024 && (1..=12).contains(&mo) && (1..=31).contains(&d));
            assert_eq!(local_stamp().len(), 19);
        }
    }
}
