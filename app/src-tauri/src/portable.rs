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
//!
//! macOS App Translocation (an unsigned app opened from a quarantined copy runs from a
//! random read-only path): the stick is invisible, so the UI explains this and asks
//! for the stick's folder; the choice is remembered in
//! `~/Library/Application Support/<bundle id>/root.txt`, the only file the app ever
//! writes outside the stick. Until a folder is chosen, the few settings go to a
//! temporary folder ([`DataMode::Translocated`]).

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
/// Recorder output folder.
pub const RECORDINGS_DIR: &str = "Recordings";
/// Exported backing tracks.
pub const EXPORTS_DIR: &str = "Exports";
/// Songs deleted from the library, kept until restored or deleted for good (the app's
/// own recycle bin: files deleted from a USB stick bypass the Windows Recycle Bin).
pub const DELETED_DIR: &str = "Deleted";
/// Studio model weights (downloaded on first use).
pub const MODELS_DIR: &str = "Models";
/// Logs, inside `Settings/`.
pub const LOGS_DIR: &str = "logs";
/// Scratch files (Studio temp, self-check renders), inside `Settings/`.
pub const TMP_DIR: &str = "tmp";
/// Settings folder (also holds the WebView2 profile and diagnostics reports).
pub const SETTINGS_DIR: &str = "Settings";
/// WebView2 user-data folder, inside `Settings/`. Named per OS so a stick used on
/// both systems never mixes web view profiles.
pub const WEBVIEW_WINDOWS_DIR: &str = "webview-windows";
/// Environment variable naming the developer's heavy-files folder (scripts/devenv.sh).
pub const DEV_ENV_VAR: &str = "JK_BUILD";
/// Sub-folder of `$JK_BUILD` used as the data root in developer mode.
pub const DEV_DATA_DIR: &str = "dev-data";
/// This crate's folder in the source tree, for the developer-machine fallbacks (the
/// dev-mode Studio and the demo song from `tests/fixtures`). The shipped build
/// (app/scripts/build-windows.sh) sets `JK_DIST_BUILD=1` at compile time, so the exe
/// carries no build-machine path (it would name the build PC's user folder) and never
/// looks for files there on someone else's computer.
pub const SOURCE_DIR: Option<&str> = match option_env!("JK_DIST_BUILD") {
    Some(_) => None,
    None => Some(env!("CARGO_MANIFEST_DIR")),
};

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
    /// macOS App Translocation and no root chosen yet: the stick is invisible, so the
    /// app keeps its few settings in a throw-away temporary folder and the UI asks the
    /// user to pick the stick's folder (WORK_ORDER section 7, trap 2).
    Translocated,
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

/// File (in the per-user app data folder, macOS only) remembering which folder the
/// user picked as the stick's root while the app was translocated. It is the only
/// thing the app ever writes outside the portable root, and only on macOS after the
/// user chose a folder (documented in app/README.md).
pub const ROOT_CHOICE_FILE: &str = "root.txt";

/// Reads a remembered root choice and checks it still looks like a stick.
pub fn read_root_choice(user_data_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(user_data_dir.join(ROOT_CHOICE_FILE)).ok()?;
    let root = PathBuf::from(text.trim());
    (root.is_absolute() && looks_like_stick_root(&root, None)).then_some(root)
}

/// Saves the user's choice of root (macOS translocation only).
pub fn write_root_choice(user_data_dir: &Path, root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(user_data_dir)?;
    crate::fs_atomic::write_atomic(
        &user_data_dir.join(ROOT_CHOICE_FILE),
        root.display().to_string().as_bytes(),
    )
}

/// True when `dir` can be the portable root the user pointed at: it holds the Mac app
/// folder or `portable.txt`, or a `Library/` folder - unless `dir` is the home folder,
/// whose `~/Library` is not a song library.
pub fn looks_like_stick_root(dir: &Path, home: Option<&Path>) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let home = home
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    let is_home = home.is_some_and(|h| h == dir);
    has_marker_file(dir) || (!is_home && has_library(dir))
}

/// Turns the folder the user picked into the root: the folder itself, or its parent
/// when the user picked the `Library` folder inside the stick.
pub fn root_from_pick(picked: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if looks_like_stick_root(picked, home) {
        return Some(picked.to_path_buf());
    }
    if name_is(picked, LIBRARY_DIR)
        && let Some(parent) = picked.parent()
        && looks_like_stick_root(parent, home)
    {
        return Some(parent.to_path_buf());
    }
    None
}

/// The data folders, all under the data root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub library: PathBuf,
    pub recordings: PathBuf,
    pub exports: PathBuf,
    pub settings: PathBuf,
    pub models: PathBuf,
    pub logs: PathBuf,
    pub tmp: PathBuf,
    /// Deleted songs (src/deleted.rs); created on the first delete.
    pub deleted: PathBuf,
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
    /// `user_data_dir` returns the operating system's per-user app data folder. It is
    /// called when neither portable nor developer mode applies, and on macOS when the
    /// app is translocated (to read the remembered root choice, `root.txt`).
    pub fn detect(
        exe: &Path,
        dev_env: Option<PathBuf>,
        user_data_dir: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<Self, String> {
        let translocated = is_translocated(exe);
        if translocated {
            // The stick is invisible from the translocated copy. Use the folder the
            // user picked earlier, or a throw-away temporary folder until they pick.
            let user = user_data_dir();
            if let Some(root) = user.as_deref().and_then(read_root_choice) {
                return Ok(Self::new(
                    DataMode::Portable,
                    Some(root.clone()),
                    root,
                    "macOS runs the app from a translocated copy; using the stick folder you chose".into(),
                    true,
                ));
            }
            let id = user
                .as_deref()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "jukeoke".into());
            return Ok(Self::new(
                DataMode::Translocated,
                None,
                std::env::temp_dir().join(format!("{id}-translocated")),
                "macOS is running the app from a temporary translocated copy, so the stick is not visible; choose the stick's folder".into(),
                true,
            ));
        }
        if let Some(found) = find_portable_root(exe, Platform::current()) {
            return Ok(Self::new(
                DataMode::Portable,
                Some(found.root.clone()),
                found.root,
                found.reason,
                false,
            ));
        }
        if let Some(dev) = dev_env.filter(|p| !p.as_os_str().is_empty()) {
            return Ok(Self::new(
                DataMode::Dev,
                None,
                dev.join(DEV_DATA_DIR),
                format!("no portable layout found; ${DEV_ENV_VAR} is set (developer machine)"),
                false,
            ));
        }
        let user = user_data_dir().ok_or("the per-user app data folder could not be determined")?;
        Ok(Self::new(
            DataMode::User,
            None,
            user,
            "no portable layout found next to the app".into(),
            false,
        ))
    }

    /// The folders under the data root (docs/APP_API.md `AppInfo.paths`).
    pub fn paths(&self) -> Paths {
        let r = &self.data_root;
        let settings = self.settings_dir.clone();
        Paths {
            library: r.join(LIBRARY_DIR),
            recordings: r.join(RECORDINGS_DIR),
            exports: r.join(EXPORTS_DIR),
            models: r.join(MODELS_DIR),
            deleted: r.join(DELETED_DIR),
            logs: settings.join(LOGS_DIR),
            tmp: settings.join(TMP_DIR),
            settings,
        }
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

/// Environment variable with which WebView2 lets anyone override the profile folder
/// the app passes in. It would move the profile off the stick, so the app removes it
/// from its own environment before any web view exists.
pub const WEBVIEW2_USER_DATA_ENV: &str = "WEBVIEW2_USER_DATA_FOLDER";

/// Removes [`WEBVIEW2_USER_DATA_ENV`] from this process's environment (Windows), so
/// the profile always goes where [`DataLayout::webview_data_dir`] says. Returns the
/// value that was ignored, for diagnostics.
///
/// # Safety (of the call site)
/// Must run before any other thread exists (environment changes are not
/// thread-safe); `lib::run` calls it first thing.
pub fn neutralise_webview2_env() -> Option<std::ffi::OsString> {
    let old = std::env::var_os(WEBVIEW2_USER_DATA_ENV)?;
    if cfg!(windows) {
        // SAFETY: called from `run()` before Tauri, the engine or any of our threads
        // start, so no other thread can be reading the environment.
        unsafe { std::env::remove_var(WEBVIEW2_USER_DATA_ENV) };
    }
    Some(old)
}

/// The WebView2 profile is disposable: the UI keeps nothing in web storage (settings
/// live in `Settings/settings.json`). When the web view cannot be created (a damaged
/// profile, or one left by a newer WebView2 on another PC), the profile folder is
/// renamed aside and deleted, and creation is retried once with a fresh profile.
///
/// Renaming first is the safe test for "nobody is using it": Windows refuses to rename
/// a folder while any file inside it is open, so a profile still held by a lingering
/// WebView2 process is left alone. Returns true when the profile was moved away.
pub fn discard_webview_profile(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    let aside = dir.with_file_name(format!(
        "{}.discarded-{:08x}",
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        crate::fs_atomic::random_u32()
    ));
    if std::fs::rename(dir, &aside).is_err() {
        return false;
    }
    let _ = std::fs::remove_dir_all(&aside);
    true
}

/// Deletes profile folders a previous [`discard_webview_profile`] could not finish
/// deleting (best effort, at start-up, in the background).
pub fn remove_discarded_profiles(settings_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(settings_dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(WEBVIEW_WINDOWS_DIR) && name.contains(".discarded-") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn root_choice_accepts_the_stick_or_its_library_but_never_home() {
        let stick = scratch("choice-stick");
        fs::create_dir_all(stick.join("Library")).unwrap();
        fs::create_dir_all(stick.join("Mac")).unwrap();
        let home = scratch("choice-home");
        fs::create_dir_all(home.join("Library")).unwrap();
        assert_eq!(root_from_pick(&stick, Some(&home)), Some(stick.clone()));
        assert_eq!(root_from_pick(&stick.join("Library"), Some(&home)), Some(stick.clone()));
        assert_eq!(root_from_pick(&home, Some(&home)), None, "~/Library is not a song library");
        assert_eq!(root_from_pick(&home.join("Library"), Some(&home)), None);
        // Remembered choice round trip.
        let user = scratch("choice-user");
        write_root_choice(&user, &stick).unwrap();
        assert_eq!(read_root_choice(&user), Some(stick.clone()));
        fs::remove_dir_all(&stick).unwrap();
        assert_eq!(read_root_choice(&user), None, "a vanished stick is not used");
        for d in [home, user] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn translocated_app_uses_the_remembered_root_or_a_temporary_folder() {
        let exe = Path::new(
            "/private/var/folders/xy/T/AppTranslocation/1234-ABCD/d/Open Mic.app/Contents/MacOS/Open Mic",
        );
        let user = scratch("transloc-user");
        let user_dir = user.join("org.jukeoke.app");
        let l = DataLayout::detect(exe, Some(PathBuf::from("/dev")), || Some(user_dir.clone())).unwrap();
        assert_eq!(l.mode, DataMode::Translocated);
        assert!(l.translocated);
        assert!(l.data_root.starts_with(std::env::temp_dir()));
        assert!(l.data_root.ends_with("org.jukeoke.app-translocated"));
        let stick = scratch("transloc-stick");
        fs::create_dir_all(stick.join("Library")).unwrap();
        write_root_choice(&user_dir, &stick).unwrap();
        let l = DataLayout::detect(exe, None, || Some(user_dir.clone())).unwrap();
        assert_eq!(l.mode, DataMode::Portable);
        assert_eq!(l.portable_root.as_deref(), Some(stick.as_path()));
        assert_eq!(l.paths().library, stick.join("Library"));
        assert_eq!(l.paths().deleted, stick.join("Deleted"));
        assert_eq!(l.paths().logs, stick.join("Settings").join("logs"));
        for d in [user, stick] {
            fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn discarding_a_profile_moves_it_away() {
        let s = scratch("discard");
        let profile = s.join(WEBVIEW_WINDOWS_DIR);
        fs::create_dir_all(profile.join("EBWebView")).unwrap();
        fs::write(profile.join("EBWebView").join("Local State"), b"{}").unwrap();
        assert!(discard_webview_profile(&profile));
        assert!(!profile.exists());
        assert!(!discard_webview_profile(&profile), "nothing left to discard");
        remove_discarded_profiles(&s);
        assert_eq!(fs::read_dir(&s).unwrap().count(), 0);
        fs::remove_dir_all(s).unwrap();
    }
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
        let exe = root.join("Windows").join("Open Mic.exe");
        touch(&exe);
        let found = find_portable_root(&exe, Platform::Windows).expect("portable");
        assert_eq!(found.root, root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn windows_stick_layout_with_marker_and_odd_case() {
        let root = scratch("win-marker");
        touch(&root.join("portable.txt"));
        let exe = root.join("WINDOWS").join("Open Mic.exe");
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
        let exe = root.join("Mac/Open Mic.app/Contents/MacOS/Open Mic");
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
        let exe = home.join("Applications/Open Mic.app/Contents/MacOS/Open Mic");
        touch(&exe);
        assert_eq!(find_portable_root(&exe, Platform::MacOs), None);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn translocation_is_detected() {
        let exe = Path::new(
            "/private/var/folders/xy/T/AppTranslocation/1234-ABCD/d/Open Mic.app/Contents/MacOS/Open Mic",
        );
        assert!(is_translocated(exe));
        assert!(!is_translocated(Path::new(
            "/Volumes/STICK/Mac/Open Mic.app/Contents/MacOS/Open Mic"
        )));
    }

    #[test]
    fn detect_prefers_portable_then_dev_then_user() {
        let root = scratch("detect");
        fs::create_dir_all(root.join("Library")).unwrap();
        let platform_dir = if Platform::current() == Platform::MacOs {
            "Mac/Open Mic.app/Contents/MacOS"
        } else {
            "Windows"
        };
        let exe = root.join(platform_dir).join("Open Mic");
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
