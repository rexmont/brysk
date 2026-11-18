//! The macOS Get Info window: the facts Finder's Get Info shows, for one item or a selection.
//!
//! Everything that touches the disk (stat, Finder tags, walking a folder for its size) happens
//! on a background thread; the window shows what has arrived and fills in the rest as it comes.
//! There are no tabs: a file or folder has one page, and a selection has the fields the selected
//! items agree on (the rest are shown as "—").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::{Entry, EntryKind, format};

use crate::image_cache::ImageCache;
use crate::t;
use crate::theme::{self, Palette};

const WIDTH: f32 = 480.;
const HEIGHT: f32 = 660.;
const MIN_WIDTH: f32 = 420.;
const MIN_HEIGHT: f32 = 460.;

/// Opens a Get Info window for `paths` (one window per call), on the display of the window in
/// front. An empty selection is a no-op.
pub fn open(paths: Vec<PathBuf>, images: Entity<ImageCache>, cx: &mut App) {
    open_with(paths, images, false, cx);
}

/// Like [`open`], but LOCKED when `locked`: the macOS window only shows facts and writes nothing,
/// so the flag changes nothing today; it is kept for the callers that share the Windows signature.
pub fn open_with(paths: Vec<PathBuf>, images: Entity<ImageCache>, _locked: bool, cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    let display = cx.active_window().and_then(|w| w.update(cx, |_, w, cx| w.display(cx).map(|d| d.id())).ok().flatten());
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(display, size(px(WIDTH), px(HEIGHT)), cx))),
        display_id: display,
        window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
        window_background: crate::theme::window_material(WindowBackgroundAppearance::MicaBackdrop),
        titlebar: Some(crate::titlebar_options()),
        // the menu item runs while the context menu is closing: this window must end up in front
        focus: true,
        ..TitleBar::window_options()
    };
    match gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| GetInfo::new(paths, images, window, cx))) {
        Ok((handle, _)) => {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
        Err(e) => log::error!("get info window: {e:#}"),
    }
}

// ---------------------------------------------------------------- the facts

/// What one selected item contributes. Only macOS facts: no shell, no drive letters.
struct Item {
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    on_disk: u64,
    created: i64,
    modified: i64,
    accessed: i64,
    mode: u32,
    uid: u32,
    gid: u32,
    readonly: bool,
    locked: bool,
    owner: String,
    group: String,
    tags: Vec<(String, u8)>,
}

/// The selection's facts, both totals and the fields every item agrees on.
#[derive(Clone)]
struct Agg {
    /// How many could be read; `count` is what the common fields are built from.
    count: usize,
    files: u64,
    dirs: u64,
    /// Bytes and allocated bytes of selected files (not of what is inside selected folders).
    bytes: u64,
    on_disk: u64,
    /// What is inside the selected folders, added as the walk reports it.
    inner_files: u64,
    inner_dirs: u64,
    inner_bytes: u64,
    inner_on_disk: u64,
    parent: Option<PathBuf>,
    same_parent: bool,
    first_is_dir: bool,
    same_kind: bool,
    first_is_symlink: bool,
    same_symlink: bool,
    first_ext: String,
    same_ext: bool,
    first_created: i64,
    same_created: bool,
    first_modified: i64,
    same_modified: bool,
    first_accessed: i64,
    same_accessed: bool,
    first_owner: String,
    same_owner: bool,
    first_uid: u32,
    same_uid: bool,
    first_group: String,
    same_group: bool,
    first_gid: u32,
    same_gid: bool,
    first_mode: u32,
    same_mode: bool,
    readonly_any: bool,
    readonly_all: bool,
    locked_any: bool,
    locked_all: bool,
    /// Finder tags every item has (an empty list is "none in common").
    tags: Option<Vec<(String, u8)>>,
    errors: Vec<String>,
    stat_done: bool,
    sizes_done: bool,
}

impl Default for Agg {
    fn default() -> Self {
        Self {
            count: 0,
            files: 0,
            dirs: 0,
            bytes: 0,
            on_disk: 0,
            inner_files: 0,
            inner_dirs: 0,
            inner_bytes: 0,
            inner_on_disk: 0,
            parent: None,
            same_parent: true,
            first_is_dir: false,
            same_kind: true,
            first_is_symlink: false,
            same_symlink: true,
            first_ext: String::new(),
            same_ext: true,
            first_created: 0,
            same_created: true,
            first_modified: 0,
            same_modified: true,
            first_accessed: 0,
            same_accessed: true,
            first_owner: String::new(),
            same_owner: true,
            first_uid: 0,
            same_uid: true,
            first_group: String::new(),
            same_group: true,
            first_gid: 0,
            same_gid: true,
            first_mode: 0,
            same_mode: true,
            readonly_any: false,
            readonly_all: false,
            locked_any: false,
            locked_all: false,
            tags: Some(Vec::new()),
            errors: Vec::new(),
            stat_done: false,
            sizes_done: false,
        }
    }
}

impl Agg {
    fn add(&mut self, path: &Path, item: &Item) {
        if self.count == 0 {
            self.parent = path.parent().map(Path::to_path_buf);
            self.first_is_dir = item.is_dir;
            self.first_is_symlink = item.is_symlink;
            self.first_ext = ext_of(path);
            self.first_created = item.created;
            self.first_modified = item.modified;
            self.first_accessed = item.accessed;
            self.first_owner = item.owner.clone();
            self.first_uid = item.uid;
            self.first_group = item.group.clone();
            self.first_gid = item.gid;
            self.first_mode = item.mode;
            self.readonly_any = item.readonly;
            self.readonly_all = item.readonly;
            self.locked_any = item.locked;
            self.locked_all = item.locked;
            self.tags = Some(item.tags.clone());
        } else {
            self.same_parent &= self.parent == path.parent().map(Path::to_path_buf);
            self.same_kind &= self.first_is_dir == item.is_dir;
            self.same_symlink &= self.first_is_symlink == item.is_symlink;
            self.same_ext &= self.first_ext == ext_of(path);
            self.same_created &= self.first_created == item.created;
            self.same_modified &= self.first_modified == item.modified;
            self.same_accessed &= self.first_accessed == item.accessed;
            self.same_owner &= self.first_owner == item.owner;
            self.same_uid &= self.first_uid == item.uid;
            self.same_group &= self.first_group == item.group;
            self.same_gid &= self.first_gid == item.gid;
            self.same_mode &= self.first_mode == item.mode;
            self.readonly_any |= item.readonly;
            self.readonly_all &= item.readonly;
            self.locked_any |= item.locked;
            self.locked_all &= item.locked;
            if let Some(tags) = &mut self.tags {
                tags.retain(|tag| item.tags.iter().any(|other| other == tag));
            }
        }
        self.count += 1;
        if item.is_dir {
            self.dirs += 1;
        } else {
            self.files += 1;
            self.bytes += item.size;
            self.on_disk += item.on_disk;
        }
    }

    fn kind_label(&self) -> String {
        if self.count == 0 {
            return "—".into();
        }
        if !self.same_kind {
            return "—".into();
        }
        if self.same_symlink && self.first_is_symlink {
            return t!("Bağlantı").to_string();
        }
        if self.first_is_dir {
            // Finder calls an .app bundle an application, not a folder.
            if self.same_ext && self.first_ext == "app" {
                return t!("Uygulama").to_string();
            }
            return t!("Klasör").to_string();
        }
        if self.same_ext {
            crate::icons::type_label_of(false, &self.first_ext)
        } else {
            "—".into()
        }
    }

    fn location(&self) -> String {
        if self.count == 0 || !self.same_parent {
            return "—".into();
        }
        match &self.parent {
            Some(p) if !p.as_os_str().is_empty() => breakable_path(p),
            _ => "/".into(),
        }
    }

    fn total_bytes(&self) -> u64 {
        self.bytes + self.inner_bytes
    }

    fn total_on_disk(&self) -> u64 {
        self.on_disk + self.inner_on_disk
    }

    fn size_text(&self) -> String {
        let mut s = format::size(self.total_bytes());
        if self.dirs > 0 && !self.sizes_done {
            s.push_str(" · ");
            s.push_str(t!("Hesaplanıyor…"));
        } else if self.count == 1 && self.files == 1 && self.total_bytes() > 0 {
            s.push_str(&format!(" ({} {})", group(self.total_bytes()), t!("bayt")));
        }
        s
    }

    fn on_disk_text(&self) -> String {
        let mut s = format::size(self.total_on_disk());
        if self.dirs > 0 && !self.sizes_done {
            s.push_str(" · ");
            s.push_str(t!("Hesaplanıyor…"));
        }
        s
    }

    fn contents_text(&self) -> String {
        if !self.sizes_done {
            return t!("Hesaplanıyor…").to_string();
        }
        let n = self.inner_files + self.inner_dirs;
        format!("{} · {} {}, {} {}", crate::i18n::items(n as usize), group(self.inner_files), t!("dosya"), group(self.inner_dirs), t!("klasör"))
    }

    fn owner_text(&self) -> String {
        if self.count == 0 || !self.same_owner || !self.same_uid {
            return "—".into();
        }
        format!("{} ({})", self.first_owner, self.first_uid)
    }

    fn group_text(&self) -> String {
        if self.count == 0 || !self.same_group || !self.same_gid {
            return "—".into();
        }
        format!("{} ({})", self.first_group, self.first_gid)
    }

    fn permissions_text(&self) -> String {
        if self.count == 0 || !self.same_mode {
            return "—".into();
        }
        format!("{} ({:o})", mode_string(self.first_mode, self.first_is_dir, self.first_is_symlink), self.first_mode & 0o7777)
    }

    fn readonly_text(&self) -> String {
        if self.count == 0 {
            "—".into()
        } else if self.readonly_all {
            t!("Evet").into()
        } else if self.readonly_any {
            "—".into()
        } else {
            t!("Hayır").into()
        }
    }

    fn locked_text(&self) -> String {
        if self.locked_all {
            t!("Evet").into()
        } else {
            "—".into()
        }
    }
}

/// One background pass over the selection. The window gets a snapshot after the first few items,
/// then every thirty-two, then as each folder's walk reports progress.
fn scan(paths: Vec<PathBuf>, cancel: Arc<AtomicBool>, tx: futures::channel::mpsc::UnboundedSender<Agg>) {
    let mut agg = Agg::default();
    let mut dirs = Vec::new();
    let mut users = HashMap::<u32, String>::new();
    let mut groups = HashMap::<u32, String>::new();

    for (i, path) in paths.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match read_item(path, &mut users, &mut groups) {
            Ok(item) => {
                if item.is_dir {
                    dirs.push(path.clone());
                }
                agg.add(path, &item);
            }
            Err(e) => agg.errors.push(format!("{}: {e}", path.display())),
        }
        agg.stat_done = i + 1 == paths.len();
        if i < 16 || (i + 1) % 32 == 0 || agg.stat_done {
            let _ = tx.unbounded_send(agg.clone());
        }
    }

    // Folder contents: the same cache the Size column and the preview pane use.
    let mut base = (0u64, 0u64, 0u64, 0u64);
    for dir in &dirs {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // The walk below treats an unreadable folder as empty; say so instead.
        if let Err(e) = std::fs::read_dir(dir) {
            agg.errors.push(format!("{}: {e}", dir.display()));
            continue;
        }
        let base_now = base;
        let cancelled = cancel.clone();
        let res = crate::preview::size_cache().get_or_compute(dir, &|| cancelled.load(Ordering::Relaxed), &mut |fs| {
            agg.inner_bytes = base_now.0 + fs.bytes;
            agg.inner_on_disk = base_now.1 + fs.allocated.unwrap_or(fs.bytes);
            agg.inner_files = base_now.2 + fs.files;
            agg.inner_dirs = base_now.3 + fs.dirs;
            let _ = tx.unbounded_send(agg.clone());
        });
        match res {
            Ok(fs) => {
                base = (base.0 + fs.bytes, base.1 + fs.allocated.unwrap_or(fs.bytes), base.2 + fs.files, base.3 + fs.dirs);
                agg.inner_bytes = base.0;
                agg.inner_on_disk = base.1;
                agg.inner_files = base.2;
                agg.inner_dirs = base.3;
            }
            Err(e) => {
                if !cancel.load(Ordering::Relaxed) {
                    agg.errors.push(format!("{}: {e}", dir.display()));
                }
            }
        }
    }
    agg.sizes_done = true;
    let _ = tx.unbounded_send(agg);
}

fn read_item(path: &Path, users: &mut HashMap<u32, String>, groups: &mut HashMap<u32, String>) -> std::io::Result<Item> {
    use std::os::macos::fs::MetadataExt as _;
    use std::os::unix::fs::MetadataExt as _;

    let m = std::fs::symlink_metadata(path)?;
    let uid = m.uid();
    let gid = m.gid();
    let owner = users.entry(uid).or_insert_with(|| user_name(uid)).clone();
    let group = groups.entry(gid).or_insert_with(|| group_name(gid)).clone();
    let flags = m.st_flags();
    Ok(Item {
        is_dir: m.is_dir(),
        is_symlink: m.file_type().is_symlink(),
        size: m.len(),
        on_disk: m.blocks() * 512,
        created: ms(m.created()),
        modified: ms(m.modified()),
        accessed: ms(m.accessed()),
        mode: m.mode(),
        uid,
        gid,
        readonly: m.permissions().readonly(),
        locked: flags & 0x0000_0002 != 0 || flags & 0x0002_0000 != 0,
        owner,
        group,
        // Finder's tags live in an extended attribute: the nearest macOS has to a tag column.
        tags: rf_win::tags::read(path),
    })
}

fn ms(t: std::io::Result<SystemTime>) -> i64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn user_name(uid: u32) -> String {
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc == 0 && !out.is_null() && !pwd.pw_name.is_null() {
        unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_string_lossy().into_owned()
    } else {
        uid.to_string()
    }
}

fn group_name(gid: u32) -> String {
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::group = std::ptr::null_mut();
    let rc = unsafe { libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc == 0 && !out.is_null() && !grp.gr_name.is_null() {
        unsafe { std::ffi::CStr::from_ptr(grp.gr_name) }.to_string_lossy().into_owned()
    } else {
        gid.to_string()
    }
}

// ---------------------------------------------------------------- small formatting helpers

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| p.display().to_string())
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// Long paths wrap at folder boundaries.
fn breakable_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s == "/" {
        return "/".into();
    }
    s.replace('/', "/\u{200B}")
}

fn date_text(ms: i64, same: bool) -> String {
    if !same || ms <= 0 {
        return "—".into();
    }
    format::datetime(ms)
}

fn group(n: u64) -> String {
    let sep = if crate::i18n::is_tr() { '.' } else { ',' };
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// "drwxr-xr-x" for the mode bits (`s`/`t` show setuid, setgid and the sticky bit).
fn mode_string(mode: u32, is_dir: bool, is_symlink: bool) -> String {
    let kind = if is_symlink {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    };
    let mut s = String::with_capacity(10);
    s.push(kind);
    for (i, mask) in [0o400u32, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001].iter().enumerate() {
        s.push(if mode & mask != 0 { ['r', 'w', 'x'][i % 3] } else { '-' });
    }
    let mut c: Vec<char> = s.chars().collect();
    if mode & 0o4000 != 0 {
        c[3] = if c[3] == 'x' { 's' } else { 'S' };
    }
    if mode & 0o2000 != 0 {
        c[6] = if c[6] == 'x' { 's' } else { 'S' };
    }
    if mode & 0o1000 != 0 {
        c[9] = if c[9] == 'x' { 't' } else { 'T' };
    }
    c.into_iter().collect()
}

/// Adds a tag once, by name: the app's colour wins over the Finder's copy of the same tag.
fn add_tag(set: &mut Vec<(String, u32)>, name: &str, color: u32) {
    if !set.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
        set.push((name.to_string(), color));
    }
}

/// A tag set in a form that can be compared (names case-folded, order-independent).
fn tag_key(set: &[(String, u32)]) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = set.iter().map(|(n, c)| (n.to_lowercase(), *c)).collect();
    v.sort();
    v
}

/// Finder's tag colours, by the index in `com.apple.metadata:_kMDItemUserTags`.
fn finder_color(index: u8) -> u32 {
    match index {
        1 => 0x8E8E93,
        2 => 0x30D158,
        3 => 0xBF5AF2,
        4 => 0x0A84FF,
        5 => 0xFFD60A,
        6 => 0xFF453A,
        7 => 0xFF9F0A,
        _ => 0xC7C7CC,
    }
}

// ---------------------------------------------------------------- the window

pub struct GetInfo {
    paths: Vec<PathBuf>,
    images: Entity<ImageCache>,
    agg: Agg,
    cancel: Arc<AtomicBool>,
    focus: FocusHandle,
    /// A thumbnail that arrives after the first frame redraws the header.
    _images: Subscription,
    /// A tag given or taken in another window redraws the tag row.
    _tags: Subscription,
}

impl Drop for GetInfo {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl GetInfo {
    fn new(paths: Vec<PathBuf>, images: Entity<ImageCache>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _images = cx.observe(&images, |_, _, cx| cx.notify());
        let _tags = cx.observe_global::<crate::tags::Tags>(|_, cx| cx.notify());
        let mut v = Self {
            paths,
            images,
            agg: Agg::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            focus: cx.focus_handle(),
            _images,
            _tags,
        };
        v.load(cx);
        window.focus(&v.focus, cx);
        window.activate_window();
        v
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let cancel = self.cancel.clone();
        let paths = self.paths.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Agg>();
        std::thread::Builder::new().name("rf-get-info".into()).spawn(move || scan(paths, cancel, tx)).ok();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(agg) = rx.next().await {
                let ok = this
                    .update(cx, |v, cx| {
                        v.agg = agg;
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
    }

    fn title(&self) -> String {
        let what = if self.paths.len() == 1 { file_name(&self.paths[0]) } else { crate::i18n::items(self.paths.len()) };
        format!("{what} — {}", t!("Özellikler"))
    }

    fn header_name(&self) -> String {
        if self.paths.len() == 1 {
            file_name(&self.paths[0])
        } else {
            crate::i18n::items(self.paths.len())
        }
    }

    fn header_subtitle(&self) -> String {
        if self.paths.len() == 1 {
            return self.agg.kind_label();
        }
        let mut s = format::size(self.agg.total_bytes());
        if self.agg.dirs > 0 && !self.agg.sizes_done {
            s.push_str(" · ");
            s.push_str(t!("Hesaplanıyor…"));
        }
        s
    }

    fn entry(&self) -> Option<Entry> {
        if self.paths.len() != 1 || self.agg.count == 0 {
            return None;
        }
        let a = &self.agg;
        let mut attrs = 0u32;
        if a.first_is_dir {
            attrs |= rf_core::attrs::DIRECTORY;
        }
        if a.first_is_symlink {
            attrs |= rf_core::attrs::REPARSE_POINT;
        }
        if a.readonly_all {
            attrs |= rf_core::attrs::READONLY;
        }
        let mut e = Entry::new(&file_name(&self.paths[0]), if a.first_is_dir { EntryKind::Dir } else { EntryKind::File }, a.total_bytes(), a.first_modified, a.first_created, attrs);
        e.link = a.first_is_symlink;
        Some(e)
    }

    fn header_icon(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let entry = self.entry();
        if let Some(e) = &entry {
            let dir = self.paths[0].parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
            if let Some(im) = self.images.update(cx, |c, cx| c.thumbnail(&dir, e, 128, cx)) {
                return img(im).size(px(64.)).object_fit(ObjectFit::Contain).into_any_element();
            }
        }
        match &entry {
            Some(e) => {
                let cat = crate::icons::category(e);
                Icon::new(crate::icons::icon_for(cat)).size(px(48.)).text_color(theme::kind_color(cat, p.dark)).into_any_element()
            }
            None => Icon::new(if self.paths.len() > 1 { L::Files } else { L::File }).size(px(48.)).text_color(p.text_secondary).into_any_element(),
        }
    }

    /// Tags to show: the app's own tag store (what the tag column shows) merged with the Finder
    /// attribute read in the background (`agg.tags` is the tags all selected items share). The
    /// second value says whether every item has exactly the same set, so an empty result can say
    /// "none" rather than "no common tag".
    fn tags(&self, cx: &App) -> (Vec<(String, u32)>, bool) {
        let store = cx.try_global::<crate::tags::Tags>().map(|t| &t.0);
        let finder = self.agg.tags.as_deref().unwrap_or(&[]);
        let mut sets: Vec<Vec<(String, u32)>> = Vec::with_capacity(self.paths.len());
        for path in &self.paths {
            let mut set = Vec::new();
            if let Some(store) = store {
                for id in store.tags_of(path) {
                    if let Some(d) = store.def(*id) {
                        add_tag(&mut set, &d.name, d.color);
                    }
                }
            }
            // The Finder attribute also carries tags given in Finder itself.
            for (name, index) in finder {
                add_tag(&mut set, name, finder_color(*index));
            }
            sets.push(set);
        }
        let Some(first) = sets.first().cloned() else { return (Vec::new(), true) };
        let first_key = tag_key(&first);
        let same = sets.iter().all(|set| tag_key(set) == first_key);
        let mut common = first;
        for set in sets.iter().skip(1) {
            common.retain(|(name, _)| set.iter().any(|(other, _)| other.eq_ignore_ascii_case(name)));
        }
        (common, same)
    }

    fn rows(&self, p: &Palette, cx: &App) -> Vec<AnyElement> {
        let a = &self.agg;
        if a.count == 0 {
            let text = if !a.stat_done {
                t!("Yükleniyor…")
            } else if a.errors.is_empty() {
                t!("Yok")
            } else {
                "—"
            };
            return vec![card(p, vec![note(text, p.text_secondary)])];
        }

        let mut rows = vec![
            row(p, t!("Tür"), a.kind_label()),
            row(p, t!("Konum"), a.location()),
            row(p, t!("Boyut"), a.size_text()),
            row(p, t!("Diskteki boyut"), a.on_disk_text()),
        ];
        if a.dirs > 0 {
            rows.push(row(p, t!("İçerik"), a.contents_text()));
        }
        rows.push(row(p, t!("Oluşturulma"), date_text(a.first_created, a.same_created)));
        rows.push(row(p, t!("Değiştirilme"), date_text(a.first_modified, a.same_modified)));
        rows.push(row(p, t!("Erişim"), date_text(a.first_accessed, a.same_accessed)));
        if !a.stat_done {
            rows.push(note(t!("Yükleniyor…"), p.text_secondary));
        }

        let mut owner = vec![
            row(p, t!("Sahip"), a.owner_text()),
            row(p, t!("Grup"), a.group_text()),
            row(p, t!("İzinler"), a.permissions_text()),
            row(p, t!("Salt okunur"), a.readonly_text()),
        ];
        if a.locked_any {
            owner.push(row(p, t!("Kilitle"), a.locked_text()));
        }

        let (tags, same_tags) = self.tags(cx);
        vec![card(p, rows), card(p, owner), card(p, vec![tags_row(p, &tags, same_tags)])]
    }

    fn error_note(&self) -> Option<String> {
        if self.agg.errors.is_empty() {
            return None;
        }
        let mut s = self.agg.errors.iter().take(3).cloned().collect::<Vec<_>>().join(" · ");
        if self.agg.errors.len() > 3 {
            s.push_str(" · …");
        }
        Some(s)
    }
}

impl Render for GetInfo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let title = self.title();
        window.set_window_title(&title);
        let icon = self.header_icon(&p, cx);
        let reveal = (self.paths.len() == 1).then(|| self.paths[0].clone());

        v_flex()
            .size_full()
            .bg(p.window_bg)
            .text_color(p.text)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, window, _| {
                if ev.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            .child(
                TitleBar::new()
                    .h(theme::TITLEBAR_H)
                    .bg(p.titlebar_bg)
                    .border_color(gpui_kit::transparent_black())
                    .child(h_flex().gap_2().items_center().text_xs().child(Icon::new(L::Info).size(px(14.))).child(title.clone())),
            )
            .child(
                v_flex()
                    .id("get-info-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_5()
                    .pt_4()
                    .pb_4()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().size(px(64.)).flex().items_center().justify_center().child(icon))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).overflow_hidden().text_ellipsis().child(self.header_name()))
                                    .child(div().text_sm().text_color(p.text_secondary).child(self.header_subtitle())),
                            ),
                    )
                    .children(self.rows(&p, cx))
                    .children(self.error_note().map(|e| note(&e, p.danger))),
            )
            .child(
                h_flex()
                    .px_5()
                    .py_3()
                    .gap_2()
                    .items_center()
                    .border_t_1()
                    .border_color(p.divider)
                    .children(reveal.map(|path| {
                        button(&p, "get-info-reveal", t!("Göster"), false, move |_, _, _| {
                            let path = path.clone();
                            std::thread::spawn(move || {
                                if let Err(e) = rf_win::shell::reveal(&path) {
                                    log::warn!("reveal in Finder: {e:#}");
                                }
                            });
                        })
                    }))
                    .child(div().flex_1())
                    .child(button(&p, "get-info-ok", t!("Tamam"), true, |_, window, _| window.remove_window())),
            )
    }
}

// ---------------------------------------------------------------- the pieces of the page

fn row(p: &Palette, k: &'static str, v: String) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .py(px(7.))
        .text_sm()
        .child(div().w(px(150.)).flex_shrink_0().text_color(p.text_secondary).child(k))
        .child(div().flex_1().min_w_0().child(v))
        .into_any_element()
}

fn card(p: &Palette, rows: Vec<AnyElement>) -> AnyElement {
    v_flex()
        .w_full()
        .px_4()
        .py_2()
        .rounded(theme::RADIUS)
        .bg(p.card_bg)
        .border_1()
        .border_color(p.border)
        .children(rows)
        .into_any_element()
}

fn note(text: &str, color: Hsla) -> AnyElement {
    div().text_xs().text_color(color).child(text.to_string()).into_any_element()
}

fn tags_row(p: &Palette, tags: &[(String, u32)], same: bool) -> AnyElement {
    let mut chips: Vec<AnyElement> = Vec::new();
    if !tags.is_empty() {
        for (name, color) in tags {
            chips.push(tag_chip(p, name, *color));
        }
    } else {
        // Every item has the same (empty) set; anything else has no tag in common.
        let text = if same { t!("Yok") } else { "—" };
        chips.push(div().text_color(p.text_secondary).child(text.to_string()).into_any_element());
    }
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .py(px(7.))
        .text_sm()
        .child(div().w(px(150.)).flex_shrink_0().text_color(p.text_secondary).child(t!("Etiketler")))
        .child(h_flex().flex_1().min_w_0().flex_wrap().gap_1p5().children(chips))
        .into_any_element()
}

fn tag_chip(p: &Palette, name: &str, color: u32) -> AnyElement {
    h_flex()
        .gap_1p5()
        .items_center()
        .px(px(7.))
        .py(px(2.))
        .rounded_full()
        .bg(p.hover)
        .border_1()
        .border_color(p.border)
        .text_xs()
        .child(div().size(px(8.)).rounded_full().bg(crate::tags::color(color)))
        .child(name.to_string())
        .into_any_element()
}

fn button(
    p: &Palette,
    id: &'static str,
    label: &'static str,
    primary: bool,
    run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .min_w(px(88.))
        .h(px(32.))
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .text_sm()
        .cursor_pointer()
        .when(primary, |d| d.bg(p.accent).text_color(p.accent_text))
        .when(!primary, |d| d.bg(p.hover).border_1().border_color(p.border))
        .hover(|s| s.opacity(0.9))
        .active(|s| s.opacity(0.8))
        .child(label)
        .on_click(move |ev, window, cx| run(ev, window, cx))
        .into_any_element()
}