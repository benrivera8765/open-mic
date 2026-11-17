// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the app keeps its data: portable mode and the fallbacks
//! (docs/ARCHITECTURE.md section 8, WORK_ORDER section 4.8).
//!
//! The USB stick looks like this:
//!
//! ```text
//! <root>/
//!   Windows/Jukeoke.exe                      the Windows app
//!   Mac/Jukeoke.app/Contents/MacOS/Jukeoke   the Mac app
//!   Library/  Recordings/  Models/  Settings/
//!   portable.txt                             (optional marker)
//! ```
//!
//! The app finds `<root>` by walking up from its own executable: on Windows the exe
//! sits in `<root>/Windows/`, on macOS the bundle sits in `<root>/Mac/`. It is in
//! portable mode when `<root>/Library/` or `<root>/portable.txt` exists, and then
//! every write goes under `<root>` (the WebView2 profile included). Nothing stores
//! the absolute root: it is found again on every start, so a new drive letter or a
//! copy on an internal disk just works.
//!
//! The walk is deliberately short and checks folder names. An unrestricted walk
//! would be dangerous on macOS: an app in `~/Applications` would find `~/Library`
//! (which every Mac has) and wrongly decide that the home folder is a portable root.
//!
//! When the app is not portable it uses a developer folder (`$JK_BUILD/dev-data`,
//! set by `scripts/devenv.sh`) or, for an ordinary install, the per-user app data
//! folder that the operating system provides.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Name of the folder that holds the Windows app inside the portable root.
pub const WINDOWS_APP_DIR: &str = "Windows";
/// Name of the folder that holds the Mac app bundle inside the portable root.
pub const MAC_APP_DIR: &str = "Mac";
/// Marker file that makes a folder a portable root even without `Library/`.
pub const PORTABLE_MARKER: &str = "portable.txt";
/// Song library folder; its presence also marks a portable root.
pub const LIBRARY_DIR: &str = "Library";
/// Settings folder (also holds the WebView2 profile and diagnostics reports).
pub const SETTINGS_DIR: &str = "Settings";
/// WebView2 user-data folder, inside `Settings/`. Named per OS so a stick used on
/// both systems never mixes web view profiles.
pub const WEBVIEW_WINDOWS_DIR: &str = "webview-windows";
/// Environment variable naming the developer's heavy-files folder (scripts/devenv.sh).
pub const DEV_ENV_VAR: &str = "JK_BUILD";
/// Sub-folder of `$JK_BUILD` used as the data root in developer mode.
pub const DEV_DATA_DIR: &str = "dev-data";

/// How the data folder was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DataMode {
    /// Everything stays under the portable root (USB stick or a copy of it).
    Portable,
    /// Developer machine: data in `$JK_BUILD/dev-data`.
    Dev,
    /// Ordinary install: data in the per-user app data folder.
    User,
}

/// A portable root found next to the executable, and why it counts as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableRoot {
    pub root: PathBuf,
    pub reason: String,
}

/// Which platform folder layout to look for. A parameter (instead of `cfg!`) so the
/// macOS rules are unit-tested on Windows and the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
}

impl Platform {
    /// The platform this binary was built for. Anything that is not macOS uses the
    /// plain "folder holding the executable" rules of Windows.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Windows
        }
    }

    fn app_dir_name(self) -> &'static str {
        match self {
            Platform::Windows => WINDOWS_APP_DIR,
            Platform::MacOs => MAC_APP_DIR,
        }
    }
}

fn name_is(path: &Path, expected: &str) -> bool {
    // exFAT and NTFS are case-insensitive, and so is the default APFS volume.
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|n| n.eq_ignore_ascii_case(expected))
}

fn has_marker_file(dir: &Path) -> bool {
    dir.join(PORTABLE_MARKER).is_file()
}

fn has_library(dir: &Path) -> bool {
    dir.join(LIBRARY_DIR).is_dir()
}

/// The folder that "holds the app": on macOS the folder containing the `.app`
/// bundle (if the executable is inside one), otherwise the folder of the executable.
fn app_container(exe: &Path, platform: Platform) -> Option<&Path> {
    if platform == Platform::MacOs
        && let Some(bundle) = exe.ancestors().skip(1).find(|p| {
            p.extension()
                .and_then(OsStr::to_str)
                .is_some_and(|e| e.eq_ignore_ascii_case("app"))
        })
    {
        return bundle.parent();
    }
    exe.parent()
}

/// Finds the portable root for an executable at `exe`, or `None` when the app is
/// not running from a portable layout.
///
/// Rules, in order:
/// 1. Stick layout: the app sits in a folder named `Windows` (Windows) or `Mac`
///    (macOS, the folder that holds `Jukeoke.app`). Its parent is the root when it
///    contains `Library/` or `portable.txt`.
/// 2. Flat layout, opt-in only: the folder holding the app contains `portable.txt`.
///    (`Library/` alone is not enough here; see the module docs about `~/Library`.)
pub fn find_portable_root(exe: &Path, platform: Platform) -> Option<PortableRoot> {
    let container = app_container(exe, platform)?;

    if name_is(container, platform.app_dir_name())
        && let Some(root) = container.parent()
    {
        if has_library(root) {
            return Some(PortableRoot {
                root: root.to_path_buf(),
                reason: format!(
                    "the app is in a '{}' folder next to a '{LIBRARY_DIR}' folder",
                    platform.app_dir_name()
                ),
            });
        }
        if has_marker_file(root) {
            return Some(PortableRoot {
                root: root.to_path_buf(),
                reason: format!(
                    "the app is in a '{}' folder next to '{PORTABLE_MARKER}'",
                    platform.app_dir_name()
                ),
            });
        }
    }

    if has_marker_file(container) {
        return Some(PortableRoot {
            root: container.to_path_buf(),
            reason: format!("'{PORTABLE_MARKER}' is next to the app"),
        });
    }
    None
}

/// True when macOS runs the app from a randomised read-only App Translocation path
/// (an unsigned app opened from a quarantined download). Such an app cannot see the
/// folder it was copied to (WORK_ORDER section 7, trap 2).
pub fn is_translocated(exe: &Path) -> bool {
    exe.to_string_lossy().contains("/AppTranslocation/")
}

/// Everything the rest of the app needs to know about where data goes.
#[derive(Debug, Clone)]
pub struct DataLayout {
    pub mode: DataMode,
    /// Set only in portable mode.
    pub portable_root: Option<PathBuf>,
    /// Holds `Settings/`, `Library/`, `Recordings/`, `Models/`.
    pub data_root: PathBuf,
    /// `<data_root>/Settings`.
    pub settings_dir: PathBuf,
    /// WebView2 user-data folder (Windows). `None` on macOS, where the web view
    /// uses a non-persistent data store and keeps nothing.
    pub webview_data_dir: Option<PathBuf>,
    /// Plain-language reason for the mode, shown in the UI and diagnostics.
    pub reason: String,
    pub translocated: bool,
}

impl DataLayout {
    fn new(
        mode: DataMode,
        portable_root: Option<PathBuf>,
        data_root: PathBuf,
        reason: String,
        translocated: bool,
    ) -> Self {
        let settings_dir = data_root.join(SETTINGS_DIR);
        let webview_data_dir = if Platform::current() == Platform::MacOs {
            None
        } else {
            Some(settings_dir.join(WEBVIEW_WINDOWS_DIR))
        };
        DataLayout {
            mode,
            portable_root,
            data_root,
            settings_dir,
            webview_data_dir,
            reason,
            translocated,
        }
    }

    /// Decides the layout for this process.
    ///
    /// `user_data_dir` is only called when neither portable nor developer mode
    /// applies; it returns the operating system's per-user app data folder.
    pub fn detect(
        exe: &Path,
        dev_env: Option<PathBuf>,
        user_data_dir: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<Self, String> {
        let translocated = is_translocated(exe);
        if let Some(found) = find_portable_root(exe, Platform::current()) {
            return Ok(Self::new(
                DataMode::Portable,
                Some(found.root.clone()),
                found.root,
                found.reason,
                translocated,
            ));
        }
        if let Some(dev) = dev_env.filter(|p| !p.as_os_str().is_empty()) {
            return Ok(Self::new(
                DataMode::Dev,
                None,
                dev.join(DEV_DATA_DIR),
                format!("no portable layout found; ${DEV_ENV_VAR} is set (developer machine)"),
                translocated,
            ));
        }
        let user = user_data_dir().ok_or("the per-user app data folder could not be determined")?;
        let reason = if translocated {
            "macOS is running the app from a temporary translocated copy, so the stick layout is not visible".to_string()
        } else {
            "no portable layout found next to the app".to_string()
        };
        Ok(Self::new(DataMode::User, None, user, reason, translocated))
    }

    /// Creates the folders this process writes to (Settings and the web view profile).
    pub fn create_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.settings_dir)?;
        if let Some(dir) = &self.webview_data_dir {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh, empty temporary folder per test (no extra crates needed).
    fn scratch(name: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "jukeoke-portable-test-{}-{n}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn windows_stick_layout_with_library() {
        let root = scratch("win-lib");
        fs::create_dir_all(root.join("Library")).unwrap();
        let exe = root.join("Windows").join("Jukeoke.exe");
        touch(&exe);
        let found = find_portable_root(&exe, Platform::Windows).expect("portable");
        assert_eq!(found.root, root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn windows_stick_layout_with_marker_and_odd_case() {
        let root = scratch("win-marker");
        touch(&root.join("portable.txt"));
        let exe = root.join("WINDOWS").join("Jukeoke.exe");
        touch(&exe);
        assert_eq!(
            find_portable_root(&exe, Platform::Windows).unwrap().root,
            root
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn library_without_the_platform_folder_is_not_portable() {
        // A dev build in target/release next to an unrelated Library folder.
        let root = scratch("win-nope");
        fs::create_dir_all(root.join("Library")).unwrap();
        let exe = root.join("release").join("jukeoke.exe");
        touch(&exe);
        assert_eq!(find_portable_root(&exe, Platform::Windows), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flat_layout_needs_the_marker_file() {
        let root = scratch("flat");
        let exe = root.join("jukeoke.exe");
        touch(&exe);
        fs::create_dir_all(root.join("Library")).unwrap();
        assert_eq!(
            find_portable_root(&exe, Platform::Windows),
            None,
            "Library alone is not enough"
        );
        touch(&root.join("portable.txt"));
        assert_eq!(
            find_portable_root(&exe, Platform::Windows).unwrap().root,
            root
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mac_stick_layout() {
        let root = scratch("mac");
        fs::create_dir_all(root.join("Library")).unwrap();
        let exe = root.join("Mac/Jukeoke.app/Contents/MacOS/Jukeoke");
        touch(&exe);
        assert_eq!(
            find_portable_root(&exe, Platform::MacOs).unwrap().root,
            root
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mac_home_folder_is_never_a_portable_root() {
        // ~/Applications/Jukeoke.app with ~/Library next to it: must NOT be portable.
        let home = scratch("mac-home");
        fs::create_dir_all(home.join("Library")).unwrap();
        let exe = home.join("Applications/Jukeoke.app/Contents/MacOS/Jukeoke");
        touch(&exe);
        assert_eq!(find_portable_root(&exe, Platform::MacOs), None);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn translocation_is_detected() {
        let exe = Path::new(
            "/private/var/folders/xy/T/AppTranslocation/1234-ABCD/d/Jukeoke.app/Contents/MacOS/Jukeoke",
        );
        assert!(is_translocated(exe));
        assert!(!is_translocated(Path::new(
            "/Volumes/STICK/Mac/Jukeoke.app/Contents/MacOS/Jukeoke"
        )));
    }

    #[test]
    fn detect_prefers_portable_then_dev_then_user() {
        let root = scratch("detect");
        fs::create_dir_all(root.join("Library")).unwrap();
        let platform_dir = if Platform::current() == Platform::MacOs {
            "Mac/Jukeoke.app/Contents/MacOS"
        } else {
            "Windows"
        };
        let exe = root.join(platform_dir).join("Jukeoke");
        touch(&exe);

        let portable =
            DataLayout::detect(&exe, Some(PathBuf::from("/dev-folder")), || None).unwrap();
        assert_eq!(portable.mode, DataMode::Portable);
        assert_eq!(portable.settings_dir, root.join("Settings"));
        if Platform::current() == Platform::Windows {
            assert_eq!(
                portable.webview_data_dir,
                Some(root.join("Settings").join("webview-windows"))
            );
        }

        let other_exe = root.join("elsewhere").join("jukeoke");
        let dev =
            DataLayout::detect(&other_exe, Some(PathBuf::from("/dev-folder")), || None).unwrap();
        assert_eq!(dev.mode, DataMode::Dev);
        assert_eq!(dev.data_root, PathBuf::from("/dev-folder").join("dev-data"));

        let user =
            DataLayout::detect(&other_exe, None, || Some(PathBuf::from("/user-data"))).unwrap();
        assert_eq!(user.mode, DataMode::User);
        assert!(DataLayout::detect(&other_exe, None, || None).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
