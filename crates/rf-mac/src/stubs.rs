//! macOS stubs for the Windows shell-replacement subsystems and for platform services whose
//! macOS implementation comes in a later milestone. They keep the names and types `rf-app`
//! uses so the Explorer UI compiles untouched; each is replaced by a real implementation when
//! its milestone comes (docs/port-macos.md).

// ------------------------------------------------------------------ tray icon (status bar)

// ------------------------------------------------------------------ taskbar progress

pub mod taskbar_progress {
    /// What a taskbar button shows; macOS shows progress in the app's own title bar instead.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum State {
        None,
        Busy,
        Normal(f32),
        Paused(f32),
        Error(f32),
    }

    pub fn set(_hwnd: isize, _state: State) {}
}

// ------------------------------------------------------------------ shell engine guard

pub mod engine_guard {
    use std::time::Duration;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Op {
        Rename,
        PermanentDelete,
        RecycleDelete,
        Copy,
        Move,
        RestoreFromBin,
        DeleteFromBin,
    }

    impl Op {
        pub fn what(self) -> &'static str {
            match self {
                Op::Rename => "renaming",
                Op::PermanentDelete => "deleting",
                Op::RecycleDelete => "deleting to the Trash",
                Op::Copy => "copying",
                Op::Move => "moving",
                Op::RestoreFromBin => "restoring from the Trash",
                Op::DeleteFromBin => "deleting from the Trash",
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct NoShellEngine {
        pub op: Op,
    }

    impl std::fmt::Display for NoShellEngine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} needs a system shell that is not running. Nothing was changed.", self.op.what())
        }
    }
    impl std::error::Error for NoShellEngine {}

    #[derive(Debug, Clone, Copy)]
    pub struct PartlyDone {
        pub op: Op,
        pub left: Option<usize>,
    }

    impl std::fmt::Display for PartlyDone {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self.left {
                Some(n) => write!(f, "{} was interrupted: {n} items were left.", self.op.what()),
                None => write!(f, "{} was interrupted.", self.op.what()),
            }
        }
    }
    impl std::error::Error for PartlyDone {}

    #[derive(Debug, Clone, Copy)]
    pub struct EngineStalled {
        pub op: Op,
        pub waited: Duration,
    }

    impl std::fmt::Display for EngineStalled {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} did not answer within {:?}.", self.op.what(), self.waited)
        }
    }
    impl std::error::Error for EngineStalled {}

    /// macOS has no runnable-shell window: Finder is always there, and the app never needs it.
    pub fn shell_present() -> bool {
        true
    }
}

// ------------------------------------------------------------------ Run-box command lookup

pub mod runcmd {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use rf_core::addrcmd::CommandLine;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Found {
        pub exe: PathBuf,
        pub args: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Started {
        Yes,
        Declined,
        Pending,
        Failed(String),
    }

    pub type ExecError = (u32, String);

    pub fn is_program_file(p: &Path) -> bool {
        std::fs::symlink_metadata(p).map(|m| !m.is_dir()).unwrap_or(false)
    }

    pub fn search_dirs() -> Vec<PathBuf> {
        let mut out = vec![
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/Applications"),
        ];
        if let Some(path) = std::env::var_os("PATH") {
            out.extend(std::env::split_paths(&path));
        }
        out
    }

    /// Finds the program the way the shell does: in the folder part when one is given, then in
    /// the usual program folders and `PATH`.
    pub fn find(readings: &[CommandLine], work_dir: &Path) -> Option<Found> {
        for r in readings {
            let prog = Path::new(&r.program);
            let candidates: Vec<PathBuf> = if prog.components().count() > 1 {
                if prog.is_absolute() {
                    vec![prog.to_path_buf()]
                } else {
                    vec![work_dir.join(prog)]
                }
            } else {
                search_dirs().into_iter().map(|d| d.join(prog)).collect()
            };
            for c in candidates {
                for ext in ["", ".app"] {
                    let mut p = c.clone();
                    if !ext.is_empty() {
                        p.set_extension(ext.trim_start_matches('.'));
                    }
                    if is_program_file(&p) {
                        return Some(Found { exe: p, args: r.args.clone() });
                    }
                }
            }
        }
        None
    }

    /// Starts the program with the open folder as its working directory.
    pub fn start(found: &Found, work_dir: &Path) -> Started {
        let mut cmd = std::process::Command::new(&found.exe);
        if !found.args.is_empty() {
            cmd.args(found.args.split_whitespace());
        }
        cmd.current_dir(work_dir);
        match cmd.spawn() {
            Ok(_) => Started::Yes,
            Err(e) => Started::Failed(e.to_string()),
        }
    }

    pub fn wait_for_start(rx: &std::sync::mpsc::Receiver<Result<(), ExecError>>, limit: Duration) -> Started {
        match rx.recv_timeout(limit) {
            Ok(Ok(())) => Started::Yes,
            Ok(Err((_, message))) => Started::Failed(message),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Started::Pending,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Started::Failed("the thread that starts programs ended without an answer".into()),
        }
    }
}

// ------------------------------------------------------------------ shell namespace

pub mod shellfolder {
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Place {
        pub parsing: String,
        pub name: String,
        pub fs_path: Option<PathBuf>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ShellChild {
        pub name: String,
        pub parsing: String,
        pub is_folder: bool,
        pub fs_path: Option<PathBuf>,
        pub size: Option<u64>,
        pub modified_ms: Option<i64>,
    }

    /// macOS has no shell namespace; a `/`-path is browsable and everything else is not.
    pub fn browsable(s: &str) -> Option<Place> {
        let p = PathBuf::from(s);
        p.is_dir().then(|| Place { parsing: s.to_string(), name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()), fs_path: Some(p) })
    }

    pub fn display_name(parsing: &str) -> Option<String> {
        let p = Path::new(parsing);
        p.file_name().map(|n| n.to_string_lossy().into_owned()).or_else(|| Some(parsing.to_string()))
    }

    pub fn list(_parsing: &str, _cancel: &dyn Fn() -> bool, _on_batch: &mut dyn FnMut(Vec<ShellChild>)) -> anyhow::Result<usize> {
        anyhow::bail!("no shell namespace on macOS")
    }

    pub fn devices() -> Vec<ShellChild> {
        Vec::new()
    }

    pub fn copy_out(_sources: &[String], _dest: &Path) -> anyhow::Result<bool> {
        Ok(false)
    }

    pub fn open(_parsing: &str) -> anyhow::Result<()> {
        anyhow::bail!("no shell namespace on macOS")
    }
}

// ------------------------------------------------------------------ document viewers

pub mod docs {
    use std::path::Path;

    use anyhow::Result;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Doc {
        Markdown(String),
        Html(String),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        Docx,
        Xlsx,
        Pptx,
        Odf,
        Rtf,
        Html,
        Sqlite,
        Minidump,
        Prefetch,
    }

    /// Office/RTF/HTML previews are Windows' own readers; macOS uses Quick Look, later.
    pub fn kind_of(_ext: &str, _head: &[u8]) -> Option<Kind> {
        None
    }

    pub type Tr<'a> = &'a dyn Fn(&'static str) -> String;

    pub fn read(_path: &Path, _kind: Kind, _tr: Tr) -> Result<Doc> {
        anyhow::bail!("document previews come with the Quick Look milestone")
    }
}

// ------------------------------------------------------------------ property columns

pub mod propcols {
    use std::collections::{HashMap, VecDeque};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use parking_lot::{Condvar, Mutex};
    use rf_core::propcols::{PropCell, PropKind, PropValue};

    #[derive(Debug, Clone)]
    pub struct PropInfo {
        pub canonical: String,
        pub display: String,
        pub kind: PropKind,
        pub default_width: u16,
        pub multiple: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Listing {
        Viewable,
        Column,
    }

    /// The property catalog is Windows' (`IShellItem2`); on macOS the app's own columns are
    /// used until a metadata (`mdls`) catalog is written.
    pub fn list_properties(_which: Listing) -> Vec<PropInfo> {
        Vec::new()
    }

    pub fn describe(_names: &[Arc<str>]) -> Vec<Option<PropInfo>> {
        Vec::new()
    }

    pub fn kind_of_vartype(_vt: u16) -> PropKind {
        PropKind::Text
    }

    pub fn is_online_only(_attributes: u32) -> bool {
        false
    }

    pub fn filetime_to_unix_ms(ticks: u64) -> i64 {
        // 100 ns ticks since 1601 → Unix ms
        ((ticks as i128 - 116_444_736_000_000_000i128) / 10_000) as i64
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ReadFail {
        Timeout,
        Unavailable,
        OnlineOnly,
    }

    pub trait PropSource: Send + Sync {
        fn read(&self, path: &Path, names: &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail>;
    }

    pub struct SystemSource {
        _limit: Duration,
        _flags: i32,
    }

    impl SystemSource {
        pub fn new(limit: Duration) -> Self {
            Self { _limit: limit, _flags: 0 }
        }
        pub fn with_flags(limit: Duration, flags: i32) -> Self {
            Self { _limit: limit, _flags: flags }
        }
    }

    impl PropSource for SystemSource {
        fn read(&self, _path: &Path, _names: &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> {
            Err(ReadFail::Unavailable)
        }
    }

    pub fn date_value(ticks: u64) -> PropValue {
        let ms = filetime_to_unix_ms(ticks);
        if ms == 0 { PropValue::Empty } else { PropValue::Int(ms) }
    }

    pub fn precheck(_attributes: u32) -> Result<(), ReadFail> {
        Ok(())
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BatchEnd {
        Finished,
        Cancelled,
        Stalled,
    }

    pub const STRIKES: usize = 3;

    pub fn read_batch<T>(
        src: &dyn PropSource,
        jobs: impl IntoIterator<Item = (T, PathBuf)>,
        names: &[Arc<str>],
        cancel: &AtomicBool,
        mut on_result: impl FnMut(T, Result<Vec<PropCell>, ReadFail>),
    ) -> BatchEnd {
        let mut strikes = 0;
        for (tag, path) in jobs {
            if cancel.load(Ordering::Relaxed) {
                return BatchEnd::Cancelled;
            }
            let r = src.read(&path, names);
            if r == Err(ReadFail::Timeout) {
                strikes += 1;
                on_result(tag, r);
                if strikes >= STRIKES {
                    return BatchEnd::Stalled;
                }
            } else {
                strikes = 0;
                on_result(tag, r);
            }
        }
        BatchEnd::Finished
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct QueueJob {
        pub path: PathBuf,
        pub size: u64,
        pub modified_ms: i64,
    }

    pub struct Done {
        pub job: QueueJob,
        pub names: Arc<[Arc<str>]>,
        pub result: Result<Vec<PropCell>, ReadFail>,
    }

    pub const STALL_PAUSE: Duration = Duration::from_secs(10);
    pub const QUEUE_WAIT: Duration = Duration::from_secs(10);

    struct QueueState {
        jobs: VecDeque<QueueJob>,
        names: Arc<[Arc<str>]>,
    }

    struct QueueShared {
        src: Arc<dyn PropSource>,
        sink: Arc<dyn Fn(Done) + Send + Sync>,
        state: Mutex<QueueState>,
        wake: Condvar,
        running: AtomicBool,
    }

    pub struct ReadQueue {
        shared: Arc<QueueShared>,
    }

    impl ReadQueue {
        pub fn new(src: Arc<dyn PropSource>, sink: impl Fn(Done) + Send + Sync + 'static) -> Self {
            Self {
                shared: Arc::new(QueueShared {
                    src,
                    sink: Arc::new(sink),
                    state: Mutex::new(QueueState { jobs: VecDeque::new(), names: Arc::from(Vec::new()) }),
                    wake: Condvar::new(),
                    running: AtomicBool::new(false),
                }),
            }
        }

        pub fn replace(&self, names: Arc<[Arc<str>]>, jobs: Vec<QueueJob>) {
            {
                let mut st = self.shared.state.lock();
                st.names = names;
                st.jobs = jobs.into();
            }
            if !self.shared.running.swap(true, Ordering::AcqRel) {
                let shared = self.shared.clone();
                let _ = std::thread::Builder::new().name("rf-propread".into()).spawn(move || loop {
                    let (job, names) = {
                        let mut st = shared.state.lock();
                        let Some(job) = st.jobs.pop_front() else {
                            shared.running.store(false, Ordering::Release);
                            shared.wake.notify_all();
                            break;
                        };
                        (job, st.names.clone())
                    };
                    let result = shared.src.read(&job.path, &names);
                    (shared.sink)(Done { job, names, result });
                });
            }
        }

        pub fn clear(&self) {
            self.shared.state.lock().jobs.clear();
        }

        pub fn waiting(&self) -> usize {
            self.shared.state.lock().jobs.len()
        }

        pub fn paused(&self) -> bool {
            false
        }
    }

    /// Reads in-process for tests and for the app's own columns.
    pub struct FakeSource {
        answer: Box<dyn Fn(&Path, &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> + Send + Sync>,
        pub calls: AtomicUsize,
    }

    impl FakeSource {
        pub fn new(answer: impl Fn(&Path, &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> + Send + Sync + 'static) -> Self {
            Self { answer: Box::new(answer), calls: AtomicUsize::new(0) }
        }
    }

    impl PropSource for FakeSource {
        fn read(&self, path: &Path, names: &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            (self.answer)(path, names)
        }
    }

    /// The columns the app offers in its chooser (none until the mdls catalog lands).
    pub fn own_columns() -> HashMap<String, PropInfo> {
        HashMap::new()
    }

    pub fn now_ms() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
    }
}

// ------------------------------------------------------------------ benchmark app kinds

pub mod bench {
    /// The programs the speed test compares (only the ones macOS can start are used).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AppKind {
        Rustyfiles,
        Explorer,
        Files,
        FilePilot,
    }

    impl AppKind {
        pub fn name(self) -> &'static str {
            match self {
                AppKind::Rustyfiles => "Brysk",
                AppKind::Explorer => "Finder",
                AppKind::Files => "Files",
                AppKind::FilePilot => "FilePilot",
            }
        }
    }
}

// ------------------------------------------------------------------ the dock's pure geometry

pub mod dock {
    use std::path::Path;

    pub use self::layout::Rect;

    pub mod spring {
        /// A spring curve with `bounce` 0..1, `x` in 0..1.
        pub fn curve_with(x: f32, bounce: f32) -> f32 {
            let b = if bounce.is_finite() { bounce.clamp(0.0, 0.6) } else { 0.0 };
            if !x.is_finite() || x <= 0.0 {
                return 0.0;
            }
            if x >= 1.0 {
                return 1.0;
            }
            let zeta = 1.0 - b;
            let omega = std::f32::consts::TAU;
            let t = x;
            if zeta >= 1.0 {
                return 1.0 - (-omega * t).exp() * (1.0 + omega * t);
            }
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            1.0 - (-zeta * omega * t).exp() * ((wd * t).cos() + (zeta * omega / wd) * (wd * t).sin())
        }

        pub fn curve(x: f32) -> f32 {
            curve_with(x, 0.0)
        }

        pub struct Spring {
            omega: f32,
            zeta: f32,
        }

        impl Spring {
            pub fn new(duration_ms: f32, bounce: f32) -> Spring {
                let d = if duration_ms.is_finite() { duration_ms.max(1.0) } else { 1.0 };
                let b = if bounce.is_finite() { bounce.clamp(0.0, 0.6) } else { 0.0 };
                Spring { omega: std::f32::consts::TAU / d, zeta: 1.0 - b }
            }
        }
    }

    pub mod magnify {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub enum Strength {
            Off,
            Subtle,
            #[default]
            Full,
        }

        impl Strength {
            pub const fn peak(self) -> f32 {
                match self {
                    Strength::Off => 1.0,
                    Strength::Subtle => 1.25,
                    Strength::Full => 1.55,
                }
            }
        }

        pub fn ease_in_out(t: f32) -> f32 {
            let t = t.clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        }
    }

    pub mod monitors {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub enum AppsWhere {
            #[default]
            Everywhere,
            WindowMonitor,
        }
    }

    pub mod layout {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct Rect {
            pub left: i32,
            pub top: i32,
            pub right: i32,
            pub bottom: i32,
        }

        impl Rect {
            pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
                Rect { left, top, right, bottom }
            }
            pub const fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Rect {
                Rect { left: x, top: y, right: x + w, bottom: y + h }
            }
            pub fn width(&self) -> i32 {
                self.right - self.left
            }
            pub fn height(&self) -> i32 {
                self.bottom - self.top
            }
            pub fn contains(&self, x: i32, y: i32) -> bool {
                x >= self.left && x < self.right && y >= self.top && y < self.bottom
            }
        }

        pub const DOCK_PADDING: i32 = 8;
        pub const HEADROOM: i32 = 24;
        pub const MIDDLE_HEIGHT: i32 = 52;
        pub const RADIUS: i32 = 14;
        pub const SEPARATOR_WIDTH: i32 = 1;

        pub fn px(v: f32) -> i32 {
            v.round() as i32
        }
        pub fn middle_width(_icons: usize) -> i32 {
            0
        }
        pub fn reserved_thickness(_scale: f32) -> i32 {
            0
        }
        pub fn shadow_on(_: bool) -> bool {
            false
        }
    }

    pub mod panel {
        use std::path::Path;

        /// No panels (dock) on macOS: menus are GPUI windows.
        pub const GAP: i32 = 6;

        pub fn any_button_down() -> bool {
            crate::input::ctrl_down() || crate::input::shift_down()
        }

        pub fn cursor() -> Option<(i32, i32)> {
            Some(crate::input::cursor_pos())
        }

        pub fn menu_orphaned(_outside: bool, _front_is_ours: bool, _open_ms: u64) -> bool {
            false
        }

        pub fn place(_: &Path, _: crate::dock::layout::Rect) -> crate::dock::layout::Rect {
            crate::dock::layout::Rect::default()
        }
    }
}
// ------------------------------------------------------------------ AppKit-free helpers

pub mod keys {
    /// The character a key makes with Shift (Windows' keyboard layout API). GPUI already gives
    /// the app the typed character on macOS, so this is only used by the Windows shortcut probe.
    pub fn shifted_char(_vk: u16) -> Option<String> {
        None
    }
}

pub mod ftp {
    use std::path::Path;

    use rf_core::Entry;

    pub fn is_ftp(p: &Path) -> bool {
        p.to_string_lossy().to_ascii_lowercase().starts_with("ftp://")
    }

    /// FTP browsing is a Windows/Internet Explorer feature here; macOS uses the mounted share
    /// or a dedicated client. The functions keep the signature and say so.
    pub fn list(_url: &str) -> std::io::Result<Vec<Entry>> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "FTP sites are not browsed yet on macOS"))
    }

    pub fn download(_url: &str, _dest: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "FTP downloads are not supported yet on macOS"))
    }

    pub fn upload(_local: &Path, _dir_url: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "FTP uploads are not supported yet on macOS"))
    }
}

pub mod rotate {
    use std::path::Path;

    /// Formats this app rotates losslessly (the same set the Windows version edits).
    pub fn can_rotate(p: &Path) -> bool {
        matches!(p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("jpg" | "jpeg" | "tif" | "tiff" | "png"))
    }

    /// The EXIF orientation after a quarter turn (1..8), the same transition table as Windows.
    pub fn next_orientation(cur: u16, clockwise: bool) -> u16 {
        match (cur, clockwise) {
            (1, true) | (8, false) => 6,
            (8, true) | (3, false) => 1,
            (3, true) | (6, false) => 8,
            (6, true) | (1, false) => 3,
            (2, true) | (4, false) => 7,
            (4, true) | (5, false) => 2,
            (5, true) | (7, false) => 4,
            (7, true) | (2, false) => 5,
            _ => cur,
        }
    }

    /// Rewrites the picture with the new orientation. Lossless EXIF rotation comes with the
    /// image-editing milestone; until then the EXIF tag is what a rotation would change and the
    /// pixels are left alone (a no-op that reports success would lie, so this fails).
    pub fn rotate(_path: &Path, _clockwise: bool) -> anyhow::Result<()> {
        anyhow::bail!("image rotation is not written yet on macOS")
    }
}

pub mod fileactions {
    use std::path::{Path, PathBuf};

    pub fn is_image(p: &Path) -> bool {
        matches!(
            p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
            Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "heif" | "avif" | "ico")
        )
    }

    pub fn is_font(p: &Path) -> bool {
        matches!(p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("ttf" | "otf" | "ttc"))
    }

    pub fn is_runnable(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        p.extension().is_some_and(|e| e == "app")
            || std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }

    /// Sets the desktop picture, as Finder's "Set Desktop Picture" does.
    pub fn set_wallpaper(path: &Path) -> anyhow::Result<()> {
        let script = format!(
            "tell application \"System Events\" to tell every desktop to set picture to POSIX file \"{}\"",
            path.display().to_string().replace('"', "\\\"")
        );
        let ok = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).status()?;
        anyhow::ensure!(ok.success(), "duvar kağıdı ayarlanamadı");
        Ok(())
    }

    /// Installs a font for the current user (`~/Library/Fonts`).
    pub fn install_font_for_user(path: &Path) -> anyhow::Result<PathBuf> {
        let dir = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("ev klasörü yok"))?.join("Library/Fonts");
        std::fs::create_dir_all(&dir)?;
        let name = path.file_name().ok_or_else(|| anyhow::anyhow!("ad yok"))?;
        let dest = crate::fileops::unique_name(&dir, &name.to_string_lossy(), "");
        std::fs::copy(path, &dest)?;
        Ok(dest)
    }
}

pub mod network {
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone)]
    pub struct Share {
        pub name: String,
        pub comment: String,
    }

    /// A server root (`\\nas` on Windows) has no macOS path form: mounted shares are `/Volumes`.
    pub fn server_root(_path: &Path) -> Option<String> {
        None
    }

    pub fn shares(_server: &str) -> std::io::Result<Vec<Share>> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "browse shares in the Finder"))
    }

    pub fn network_shortcuts() -> Vec<(String, PathBuf)> {
        Vec::new()
    }

    pub fn map_drive_dialog(_owner: isize) {}

    pub fn disconnect_drive_dialog(_owner: isize) {}
}

pub mod complete {
    use std::path::{Path, PathBuf};

    /// Path completions for the address bar: the folders of `input`'s folder that start with its
    /// last part, plus the literal folder names.
    pub fn complete_dirs(input: &str, limit: usize, include_hidden: bool) -> Vec<PathBuf> {
        let input = input.trim().trim_matches('"');
        let p = Path::new(input);
        let (dir, prefix) = if input.ends_with('/') {
            (p.to_path_buf(), String::new())
        } else {
            (p.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")), p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        };
        let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
        let prefix = prefix.to_lowercase();
        let mut out: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter(|e| include_hidden || !e.file_name().to_string_lossy().starts_with('.'))
            .filter(|e| {
                prefix.is_empty() || {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    n.starts_with(&prefix)
                }
            })
            .map(|e| e.path())
            .collect();
        out.truncate(limit);
        out
    }

    pub fn completable(p: &str) -> bool {
        let p = p.trim();
        Path::new(p).is_absolute() || p == "~" || p.starts_with("~/")
    }
}

pub mod apps {
    use std::path::Path;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct App {
        pub name: String,
        pub id: String,
    }

    impl App {
        pub fn parsing(&self) -> String {
            self.id.clone()
        }
    }

    /// The applications in the usual places (`/Applications`, `/System/Applications`, `~/Applications`).
    pub fn list() -> anyhow::Result<Vec<App>> {
        let mut out = Vec::new();
        for dir in ["/Applications", "/System/Applications", "/Applications/Utilities", "/System/Applications/Utilities"] {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "app") {
                        out.push(App { name: p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), id: p.display().to_string() });
                    }
                }
            }
        }
        if let Some(home) = dirs::home_dir() {
            if let Ok(rd) = std::fs::read_dir(home.join("Applications")) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "app") {
                        out.push(App { name: p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), id: p.display().to_string() });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(out)
    }

    pub fn launch(app: &App) -> anyhow::Result<()> {
        let p = Path::new(&app.id);
        if p.exists() {
            crate::shell::open(p)
        } else {
            anyhow::bail!("uygulama bulunamadı")
        }
    }

    pub fn launch_directly_for_probe(app: &App) -> anyhow::Result<()> {
        launch(app)
    }
}

pub mod apppkg {
    use std::path::Path;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        Msix,
        MsixBundle,
        Apk,
        Ipa,
        Nuget,
    }

    #[derive(Debug, Clone, Default)]
    pub struct AppInfo {
        pub name: Option<String>,
        pub id: Option<String>,
        pub version: Option<String>,
        pub publisher: Option<String>,
        pub arch: Vec<String>,
        pub min_os: Option<String>,
        pub permissions: u32,
        pub description: Option<String>,
        pub icon_png: Option<Vec<u8>>,
    }

    /// Package formats: Windows has MSIX; Android/iOS packages open like archives.
    pub fn kind_of(_path: &Path) -> Option<Kind> {
        None
    }

    pub fn info(_path: &Path) -> Option<(Kind, AppInfo)> {
        None
    }

    pub fn icon(_bytes: &[u8], _max: u32) -> Option<crate::thumbs::Bgra> {
        None
    }
}

pub mod editors {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Editor {
        pub name: &'static str,
        pub exe: std::path::PathBuf,
    }

    pub fn installed() -> Vec<Editor> {
        Vec::new()
    }

    pub fn name_of<'a>(editors: &'a [Editor], exe: &std::path::Path) -> Option<&'a str> {
        editors.iter().find(|e| e.exe == exe).map(|e| e.name)
    }

    pub fn peek() -> (Vec<Editor>, bool) {
        (Vec::new(), false)
    }

    pub fn refresh() -> bool {
        false
    }
}

pub mod libraries {
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone)]
    pub struct Library {
        pub file: PathBuf,
        pub name: String,
        pub folders: Vec<PathBuf>,
        pub default_save: Option<PathBuf>,
    }

    /// macOS has no Windows Libraries; the Finder's sidebar and the app's own Shelf take their
    /// place. The list is empty so nothing false appears.
    pub fn list() -> Vec<Library> {
        Vec::new()
    }

    pub fn load(_file: &Path) -> Option<Library> {
        None
    }
}

pub mod shortcut {
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct ShortcutInfo {
        pub target: PathBuf,
        pub args: String,
        pub working_dir: PathBuf,
        pub icon: String,
        pub icon_index: i32,
    }

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Details {
        pub target: String,
        pub args: String,
        pub working_dir: String,
        pub comment: String,
        pub show: i32,
        pub hotkey: String,
    }

    /// A symlink (macOS' nearest simple shortcut) resolves to its target; an alias needs its
    /// bookmark and arrives with the Finder-integration milestone.
    pub fn resolve(path: &Path) -> anyhow::Result<ShortcutInfo> {
        let target = std::fs::read_link(path)?;
        Ok(ShortcutInfo { target, ..ShortcutInfo::default() })
    }

    pub fn create(link_path: &Path, target: &Path) -> anyhow::Result<()> {
        std::os::unix::fs::symlink(target, link_path)?;
        Ok(())
    }

    pub fn details(path: &Path) -> anyhow::Result<Details> {
        let info = resolve(path)?;
        Ok(Details { target: info.target.display().to_string(), ..Details::default() })
    }

    pub fn save(path: &Path, d: Details) -> anyhow::Result<()> {
        let target = PathBuf::from(d.target);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        create(path, &target)
    }
}

pub mod wsearch {
    use std::path::Path;

    use crate::search::SearchHit;

    /// macOS' Spotlight is not searched here yet (the name index and the walk are).
    pub fn is_indexed(_root: &Path) -> bool {
        false
    }

    pub fn search(
        _root: &Path,
        _query: &str,
        _max_results: usize,
        _cancelled: &dyn Fn() -> bool,
        _on_batch: &mut dyn FnMut(Vec<SearchHit>),
    ) -> anyhow::Result<usize> {
        Ok(0)
    }
}

pub mod ownfolder {
    /// Windows cannot update a running program; the app leaves its install folder. macOS can
    /// replace a `.app` while it runs, so there is nothing to leave.
    pub fn leave() {}
}

pub mod pe {
    use std::path::Path;

    #[derive(Debug, Clone, Default)]
    pub struct PeInfo {
        pub arch: &'static str,
        pub version: Option<String>,
        pub product: Option<String>,
        pub company: Option<String>,
        pub description: Option<String>,
        pub product_version: Option<String>,
        pub copyright: Option<String>,
        pub original_filename: Option<String>,
        pub language: Option<String>,
        pub trademarks: Option<String>,
        pub internal_name: Option<String>,
    }

    /// Windows executables are not run here; their version resource is still readable by a
    /// future parser. Until then no facts are shown rather than wrong ones.
    pub fn info(_path: &Path) -> Option<PeInfo> {
        None
    }

    pub fn is_pe(_path: &Path) -> bool {
        false
    }
}

pub mod cert {
    use std::path::Path;

    #[derive(Debug, Clone, Default)]
    pub struct CertInfo {
        pub subject: String,
        pub issuer: String,
        pub not_before: i64,
        pub not_after: i64,
    }

    /// Certificate files are parsed with the Security framework in a later milestone.
    pub fn info(_path: &Path) -> Option<CertInfo> {
        None
    }
}

pub mod volume {
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, Default)]
    pub struct VolumeInfo {
        pub root: PathBuf,
        pub label: String,
        pub filesystem: String,
        pub serial: u32,
        pub total: u64,
        pub free: u64,
        pub drive_type: String,
    }

    impl VolumeInfo {
        pub fn serial_string(&self) -> String {
            format!("{:04X}-{:04X}", self.serial >> 16, self.serial & 0xFFFF)
        }
    }

    pub fn volume_info(root: &Path) -> std::io::Result<VolumeInfo> {
        let d = crate::drives::mount_of(root).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "birim bulunamadı"))?;
        Ok(VolumeInfo {
            root: d.root,
            label: d.label,
            filesystem: d.fs,
            serial: 0,
            total: d.total,
            free: d.free,
            drive_type: match d.kind {
                crate::drives::DriveKind::Network => "Ağ birimi",
                crate::drives::DriveKind::Removable => "Çıkarılabilir",
                _ => "Yerel birim",
            }
            .to_string(),
        })
    }

    /// Renaming a volume needs `diskutil` and the user's permission; the properties page offers
    /// it only where the system allows.
    pub fn set_label(_path: &Path, _label: &str) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "birim adı burada değiştirilemez"))
    }
}

pub mod props {
    use std::path::Path;

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Editable {
        pub title: String,
        pub subject: String,
        pub tags: String,
        pub comment: String,
        pub authors: String,
    }

    /// Media facts for the preview/search (`mdls` in a later milestone; the decoders the app
    /// already has give the important ones).
    pub fn media_props(_path: &Path) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    pub fn editable(_path: &Path) -> Option<Editable> {
        None
    }

    pub fn set_editable(_path: &Path, _old: &Editable, _new: &Editable) -> anyhow::Result<()> {
        anyhow::bail!("file properties are not written yet on macOS")
    }
}

pub mod sysparams {
    /// macOS always animates windows; the app's own switch is the one that matters.
    pub fn client_area_animation() -> bool {
        true
    }
}

pub mod nameindex {
    use std::path::{Path, PathBuf};

    pub fn set_excluded(dirs: &[PathBuf]) {
        crate::indexer::set_excluded(dirs);
    }

    pub fn default_excluded_names() -> &'static [&'static str] {
        crate::indexer::default_excluded_names()
    }

    pub fn status() -> Vec<(PathBuf, u8, usize)> {
        crate::indexer::status().into_iter().map(|(root, state, live, _)| (root, state as u8, live)).collect()
    }

    pub fn start(roots: Vec<PathBuf>, dir: PathBuf) {
        crate::indexer::start(roots, dir);
    }

    pub fn stop() {
        crate::indexer::stop();
    }

    pub fn search(pattern: &str, under: &Path, include_hidden: bool, max: usize) -> Option<Vec<PathBuf>> {
        crate::indexer::search(pattern, under, include_hidden, max)
    }
}

pub mod dialog_reg {
    pub const CLASSES: &str = "";
    pub const DLL_NAME: &str = "Brysk_dialog.dylib";

    pub fn is_enabled(_classes_root: &str) -> bool {
        false
    }

    pub fn point_to(_dll: &str, _classes_root: &str) -> bool {
        false
    }

    pub fn enable(_dll: &str, _classes_root: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn disable(_classes_root: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

pub mod default_fm {
    pub const APP_NAME: &str = "Brysk";
    pub const CLASSES: &str = "";

    /// macOS does not let an app become the Finder; the share of folder-opening goes through
    /// LaunchServices handlers, which come with the packaging milestone.
    pub fn is_enabled(_classes_root: &str) -> bool {
        false
    }

    pub fn enable(_exe: &str, _classes_root: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn disable(_classes_root: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn heal(_current: &str, _classes_root: &str) -> bool {
        false
    }

    pub fn add_archives(_exe: &str, _classes_root: &str) -> bool {
        false
    }

    pub fn forwarded_recently(_arg: &str) -> bool {
        false
    }

    pub fn note_forward(_arg: &str) {}
}

pub mod vfiles {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    pub struct Progress<'a> {
        pub bytes: (u64, u64),
        pub files: (usize, usize),
        pub current: Option<&'a str>,
    }

    #[derive(Debug, Default)]
    pub struct Pasted {
        pub arrived: Vec<PathBuf>,
        pub failed: usize,
        pub first_error: Option<String>,
        pub cancelled: bool,
    }

    /// Promised/virtual files on the pasteboard (NSFilePromiseReceiver) come with the Finder
    /// drag-and-drop milestone; ordinary file URLs are `clipboard`.
    pub fn clipboard_has() -> bool {
        false
    }

    pub fn paste_clipboard(_dest: &Path, _progress: &mut dyn FnMut(Progress), _cancel: &AtomicBool) -> anyhow::Result<Pasted> {
        anyhow::bail!("promised files are not pasted yet on macOS")
    }
}

pub mod share {
    use std::path::PathBuf;

    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSSharingServicePicker};
    use objc2::runtime::AnyObject;
    use objc2_foundation::{MainThreadMarker, NSArray, NSRect, NSRectEdge, NSSize, NSPoint, NSString, NSURL};

    /// The items a share sheet would offer.
    pub struct Prepared {
        pub paths: Vec<PathBuf>,
    }

    pub fn prepare(paths: &[PathBuf]) -> anyhow::Result<Prepared> {
        Ok(Prepared { paths: paths.to_vec() })
    }

    /// The macOS share sheet (`NSSharingServicePicker`), anchored to the key window.
    pub fn show(_hwnd: isize, items: Prepared, _title: &str) -> anyhow::Result<()> {
        if items.paths.is_empty() {
            return Ok(());
        }
        let paths = items.paths;
        dispatch2::DispatchQueue::main().exec_async(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            let urls: Vec<Retained<NSURL>> = paths.iter().map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))).collect();
            let urls = NSArray::from_retained_slice(&urls);
            let array: &NSArray<AnyObject> = unsafe { &*(&*urls as *const NSArray<NSURL> as *const NSArray<AnyObject>) };
            let picker = unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), array) };
            let app = NSApplication::sharedApplication(mtm);
            let Some(window) = app.keyWindow() else { return };
            let Some(view) = window.contentView() else { return };
            unsafe {
                picker.showRelativeToRect_ofView_preferredEdge(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)), &view, NSRectEdge::MaxY);
            }
            // The picker owns itself while its menu is up; leaking one small object per share
            // keeps it alive for the whole interaction.
            std::mem::forget(picker);
        });
        Ok(())
    }
}

pub mod runfind {
    pub use crate::stubs::runcmd::{Found, find, is_program_file, search_dirs};
}

pub mod jumplist {
    use std::path::{Path, PathBuf};

    pub const NEW_WINDOW_ARG: &str = "--new-window";

    /// (category name, items: (label, path)).
    pub type Category = (String, Vec<(String, PathBuf)>);

    /// The Dock's menu is the macOS counterpart; it is written by the packaging milestone.
    pub fn set(_exe: &Path, _tasks: &[(&str, &str)], _categories: &[Category]) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn busy(_hresult: u32) -> bool {
        false
    }
}

pub mod recycle_settings {
    use rf_core::recycle_bin::{BinSettings, BinSettingsStore, Changes};

    /// macOS keeps no per-volume Trash size policy; the dialog says so instead of writing
    /// Windows registry keys.
    pub struct SystemBinSettings;

    impl BinSettingsStore for SystemBinSettings {
        fn read(&self) -> anyhow::Result<BinSettings> {
            Ok(BinSettings { volumes: Vec::new(), confirm_delete: true, confirm_locked: true })
        }

        fn write(&self, _changes: &Changes) -> anyhow::Result<()> {
            anyhow::bail!("Trash settings are not written on macOS")
        }
    }
}

pub mod removal {
    /// "Safely remove" is a Windows shell notification; macOS unmounts volumes itself.
    pub fn start(_on_request: impl Fn(char, std::sync::mpsc::Sender<()>) + Send + 'static) {}
}
