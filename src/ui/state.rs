//! The results of one scan as the window sees them: the files, the folder
//! tree, what is ticked, and which files pass the current filters.
//! No drawing happens here, so this part is unit-tested.

use crate::model::{Category, Chance, FoundFile, SourceSpec, Status};
use crate::scan::{ResumePoint, ScanHandle, ScanOptions};
use crate::source::Source;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

pub struct DirNode {
    pub name: String,
    pub parent: u32,
    pub children: Vec<u32>,
}

/// All folders that appear in the results. Node 0 is the root.
pub struct Dirs {
    pub nodes: Vec<DirNode>,
    pub paths: Vec<String>,
    by_path: HashMap<String, u32>,
}

impl Default for Dirs {
    fn default() -> Self {
        Dirs {
            nodes: vec![DirNode {
                name: String::new(),
                parent: 0,
                children: Vec::new(),
            }],
            paths: vec![String::new()],
            by_path: HashMap::from([(String::new(), 0)]),
        }
    }
}

impl Dirs {
    /// The node for a path, creating it and its parents as needed.
    pub fn intern(&mut self, path: &str) -> u32 {
        if let Some(id) = self.by_path.get(path) {
            return *id;
        }
        let (parent_path, name) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path),
        };
        let parent = self.intern(parent_path);
        let id = self.nodes.len() as u32;
        self.nodes.push(DirNode {
            name: name.to_string(),
            parent,
            children: Vec::new(),
        });
        self.paths.push(path.to_string());
        self.nodes[parent as usize].children.push(id);
        self.by_path.insert(path.to_string(), id);
        id
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
    Name,
    Size,
    Modified,
    Chance,
    Folder,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SizeFilter {
    #[default]
    Any,
    Under1Mb,
    From1To100Mb,
    Over100Mb,
}

impl SizeFilter {
    pub const ALL: [SizeFilter; 4] = [
        SizeFilter::Any,
        SizeFilter::Under1Mb,
        SizeFilter::From1To100Mb,
        SizeFilter::Over100Mb,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SizeFilter::Any => "Any size",
            SizeFilter::Under1Mb => "Under 1 MB",
            SizeFilter::From1To100Mb => "1 MB to 100 MB",
            SizeFilter::Over100Mb => "Over 100 MB",
        }
    }
    fn pass(self, size: u64) -> bool {
        const MB: u64 = 1 << 20;
        match self {
            SizeFilter::Any => true,
            SizeFilter::Under1Mb => size < MB,
            SizeFilter::From1To100Mb => (MB..=100 * MB).contains(&size),
            SizeFilter::Over100Mb => size > 100 * MB,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DateFilter {
    #[default]
    Any,
    Last7Days,
    Last30Days,
    LastYear,
    Older,
}

impl DateFilter {
    pub const ALL: [DateFilter; 5] = [
        DateFilter::Any,
        DateFilter::Last7Days,
        DateFilter::Last30Days,
        DateFilter::LastYear,
        DateFilter::Older,
    ];
    pub fn label(self) -> &'static str {
        match self {
            DateFilter::Any => "Any date",
            DateFilter::Last7Days => "Changed in the last 7 days",
            DateFilter::Last30Days => "Changed in the last 30 days",
            DateFilter::LastYear => "Changed in the last year",
            DateFilter::Older => "Older than a year",
        }
    }
    fn pass(self, modified: i64, now: i64) -> bool {
        const DAY: i64 = 86_400;
        match self {
            DateFilter::Any => true,
            // Files found by content have no date: they only match "any".
            _ if modified <= 0 => false,
            DateFilter::Last7Days => modified >= now - 7 * DAY,
            DateFilter::Last30Days => modified >= now - 30 * DAY,
            DateFilter::LastYear => modified >= now - 365 * DAY,
            DateFilter::Older => modified < now - 365 * DAY,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ChanceFilter {
    #[default]
    Any,
    Good,
    GoodOrPartial,
}

impl ChanceFilter {
    pub const ALL: [ChanceFilter; 3] = [
        ChanceFilter::Any,
        ChanceFilter::Good,
        ChanceFilter::GoodOrPartial,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ChanceFilter::Any => "Any recoverability",
            ChanceFilter::Good => "Good only",
            ChanceFilter::GoodOrPartial => "Good or partly overwritten",
        }
    }
    fn pass(self, c: Chance) -> bool {
        match self {
            ChanceFilter::Any => true,
            ChanceFilter::Good => c == Chance::Good,
            ChanceFilter::GoodOrPartial => c != Chance::Poor,
        }
    }
}

/// Speed and time-left estimate from recent progress.
#[derive(Default)]
pub struct Meter {
    samples: VecDeque<(Instant, u64)>,
}

impl Meter {
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    pub fn sample(&mut self, done: u64) {
        let now = Instant::now();
        if self
            .samples
            .back()
            .map(|(t, _)| now.duration_since(*t).as_millis() < 250)
            .unwrap_or(false)
        {
            return;
        }
        self.samples.push_back((now, done));
        while self.samples.len() > 40 {
            self.samples.pop_front();
        }
    }

    /// Bytes per second over the last few seconds.
    pub fn speed(&self) -> Option<f64> {
        let (first, last) = (self.samples.front()?, self.samples.back()?);
        let secs = last.0.duration_since(first.0).as_secs_f64();
        if secs < 0.8 || last.1 <= first.1 {
            return None;
        }
        Some((last.1 - first.1) as f64 / secs)
    }

    /// Seconds left for `remaining` bytes.
    pub fn eta(&self, remaining: u64) -> Option<u64> {
        self.speed().map(|s| (remaining as f64 / s).ceil() as u64)
    }
}

fn cmp_ci(a: &str, b: &str) -> Ordering {
    let mut x = a.chars().flat_map(char::to_lowercase);
    let mut y = b.chars().flat_map(char::to_lowercase);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(p), Some(q)) if p != q => return p.cmp(&q),
            _ => {}
        }
    }
}

/// Does `text` match a pattern with `*` (any run of characters) and `?`
/// (any one character)?
fn glob(pat: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    pat[p..].iter().all(|c| *c == '*')
}

/// One word of the search box.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Term {
    /// The name contains this text.
    Has(String),
    /// The whole name matches this pattern (`*.jpg`, `img_20??.*`).
    Glob(Vec<char>),
}

/// Splits what was typed into words; every word has to match.
fn parse_search(search: &str) -> Vec<Term> {
    search
        .split_whitespace()
        .map(|w| {
            let w = w.to_lowercase();
            if w.contains(['*', '?']) {
                Term::Glob(w.chars().collect())
            } else {
                Term::Has(w)
            }
        })
        .collect()
}

fn matches_search(terms: &[Term], name: &str) -> bool {
    if terms.is_empty() {
        return true;
    }
    let name = name.to_lowercase();
    let mut chars: Option<Vec<char>> = None;
    terms.iter().all(|t| match t {
        Term::Has(w) => name.contains(w.as_str()),
        Term::Glob(p) => glob(p, chars.get_or_insert_with(|| name.chars().collect())),
    })
}

/// One field of a CSV line, quoted when needed. Text that a spreadsheet
/// would run as a formula is defused with a leading apostrophe.
fn csv_field(text: &str) -> String {
    let risky = text.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let text = if risky {
        format!("'{text}")
    } else {
        text.to_string()
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn chance_rank(c: Chance) -> u8 {
    match c {
        Chance::Good => 0,
        Chance::Partial => 1,
        Chance::Unknown => 2,
        Chance::Poor => 3,
    }
}

/// Everything about the scan that is open in the Results tab.
pub struct Session {
    pub spec: SourceSpec,
    pub label: String,
    /// `None` when a saved session was opened and its drive is not available.
    pub source: Option<Source>,
    pub source_note: String,
    pub source_len: u64,
    pub disks: Vec<u32>,
    pub options: ScanOptions,

    pub files: Vec<FoundFile>,
    pub dir_of: Vec<u32>,
    cats: Vec<u8>,
    pub checked: Vec<bool>,
    pub dirs: Dirs,
    pub log: Vec<String>,

    pub scan: Option<ScanHandle>,
    pub finished: bool,
    pub stopped: bool,
    pub meter: Meter,
    pub started: Instant,
    pub elapsed_secs: Option<u64>,
    pub saved: bool,

    // Filters and view.
    pub cat: Option<Category>,
    pub size_filter: SizeFilter,
    pub date_filter: DateFilter,
    pub chance_filter: ChanceFilter,
    pub show_existing: bool,
    pub folder: u32,
    pub sort: (Col, bool),
    pub grid: bool,
    search: String,
    terms: Vec<Term>,
    now: i64,

    /// Where a stopped deep scan can be picked up again.
    pub resume: Option<ResumePoint>,
    /// Entries that were already listed before the scan was continued:
    /// the continued scan reads the file tables again and must not list
    /// them twice.
    seen: HashSet<u64>,
    /// The highlighted row should be brought into view (keyboard moves).
    pub scroll_to_highlight: bool,
    /// Scroll position and height of the list, as last drawn.
    pub list_view: (f32, f32),
    /// Tiles per row of the thumbnail grid, as last drawn.
    pub per_row: usize,

    // Derived by `rebuild`.
    pub visible: Vec<u32>,
    passing: Vec<bool>,
    pub cat_counts: [usize; 6],
    pub dir_total: Vec<u32>,
    pub dir_checked: Vec<u32>,
    pub tree_rows: Vec<(u32, u16)>,
    pub dirty: bool,
    last_build: Instant,

    pub sel_count: u64,
    pub sel_bytes: u64,
    pub highlighted: Option<u32>,
    pub expanded: HashSet<u32>,
}

impl Session {
    pub fn new(
        spec: SourceSpec,
        label: String,
        source: Option<Source>,
        options: ScanOptions,
    ) -> Session {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Session {
            spec,
            label,
            source_len: source.as_ref().map(|s| s.len()).unwrap_or(0),
            source,
            source_note: String::new(),
            disks: Vec::new(),
            show_existing: options.include_existing,
            options,
            files: Vec::new(),
            dir_of: Vec::new(),
            cats: Vec::new(),
            checked: Vec::new(),
            dirs: Dirs::default(),
            log: Vec::new(),
            scan: None,
            finished: false,
            stopped: false,
            meter: Meter::default(),
            started: Instant::now(),
            elapsed_secs: None,
            saved: false,
            cat: None,
            size_filter: SizeFilter::Any,
            date_filter: DateFilter::Any,
            chance_filter: ChanceFilter::Any,
            folder: 0,
            sort: (Col::Folder, true),
            grid: false,
            search: String::new(),
            terms: Vec::new(),
            now,
            resume: None,
            seen: HashSet::new(),
            scroll_to_highlight: false,
            list_view: (0.0, 0.0),
            per_row: 1,
            visible: Vec::new(),
            passing: Vec::new(),
            cat_counts: [0; 6],
            dir_total: vec![0],
            dir_checked: vec![0],
            tree_rows: Vec::new(),
            dirty: true,
            last_build: Instant::now(),
            sel_count: 0,
            sel_bytes: 0,
            highlighted: None,
            expanded: HashSet::new(),
        }
    }

    pub fn scanning(&self) -> bool {
        self.scan.is_some() && !self.finished
    }

    pub fn add_files(&mut self, files: Vec<FoundFile>) {
        for f in files {
            if !self.seen.is_empty() && self.seen.contains(&identity(&f)) {
                continue;
            }
            let dir = self.dirs.intern(&f.path);
            // Open the first two levels of the tree as they appear.
            let parent = self.dirs.nodes[dir as usize].parent;
            if parent == 0 || self.dirs.nodes[parent as usize].parent == 0 {
                self.expanded.insert(parent);
            }
            self.dir_of.push(dir);
            self.cats.push(f.category().index() as u8);
            self.checked.push(false);
            self.files.push(f);
        }
        self.dirty = true;
        self.saved = false;
    }

    /// Drops content-only entries that a later file system record made redundant.
    pub fn retract(&mut self, offsets: &HashSet<u64>) {
        let keep: Vec<bool> = self
            .files
            .iter()
            .map(|f| !crate::scan::is_retracted(f, offsets))
            .collect();
        let mut it = keep.iter();
        self.files.retain(|_| *it.next().unwrap_or(&true));
        {
            let v = &mut self.dir_of;
            let mut it = keep.iter();
            v.retain(|_| *it.next().unwrap_or(&true));
        }
        let mut it = keep.iter();
        self.cats.retain(|_| *it.next().unwrap_or(&true));
        let mut it = keep.iter();
        self.checked.retain(|_| *it.next().unwrap_or(&true));
        self.highlighted = None;
        self.recount_selection();
        self.rebuild_now();
    }

    fn recount_selection(&mut self) {
        self.sel_count = 0;
        self.sel_bytes = 0;
        for (i, c) in self.checked.iter().enumerate() {
            if *c {
                self.sel_count += 1;
                self.sel_bytes += self.files[i].size;
            }
        }
    }

    pub fn set_search(&mut self, search: &str) {
        let s = search.trim().to_lowercase();
        if s != self.search {
            self.terms = parse_search(&s);
            self.search = s;
            self.rebuild_now();
        }
    }

    /// Is anything narrowing the list (besides the folder)?
    pub fn filters_active(&self) -> bool {
        self.cat.is_some()
            || self.size_filter != SizeFilter::Any
            || self.date_filter != DateFilter::Any
            || self.chance_filter != ChanceFilter::Any
    }

    /// Back to "everything": all types, sizes, dates and ratings.
    pub fn clear_filters(&mut self) {
        self.cat = None;
        self.size_filter = SizeFilter::Any;
        self.date_filter = DateFilter::Any;
        self.chance_filter = ChanceFilter::Any;
        self.rebuild_now();
    }

    /// Remembers what is listed so that a continued scan adds only new entries.
    pub fn begin_resume(&mut self) {
        self.seen = self.files.iter().map(identity).collect();
        self.finished = false;
        self.stopped = false;
        self.elapsed_secs = None;
        self.started = Instant::now();
        self.meter.reset();
    }

    /// Ticks or clears every shown file with this extension.
    pub fn set_ext(&mut self, ext: &str, on: bool) {
        for k in 0..self.visible.len() {
            let i = self.visible[k] as usize;
            if self.files[i].ext() == ext {
                self.set_one(i, on);
            }
        }
        self.recount_dirs();
    }

    /// The list as a CSV table (for a spreadsheet): the ticked files, or
    /// everything shown when nothing is ticked. Returns the text and the
    /// number of files in it.
    pub fn csv(&self, tz: i32) -> (String, usize) {
        let rows: Vec<u32> = if self.sel_count > 0 {
            self.selected_indices()
        } else {
            self.visible.clone()
        };
        // The byte-order mark makes Excel read the names as UTF-8.
        let mut out = String::from(
            "\u{feff}Name,Folder,Size in bytes,Changed,Created,Status,Recoverability,Found through,Note\r\n",
        );
        for i in &rows {
            let f = &self.files[*i as usize];
            let line = [
                csv_field(&f.name),
                csv_field(&f.path),
                f.size.to_string(),
                csv_field(&crate::util::fmt_time(f.modified, tz)),
                csv_field(&crate::util::fmt_time(f.created, tz)),
                f.status.label().to_string(),
                f.chance.label().to_string(),
                csv_field(f.origin.label()),
                csv_field(&f.note),
            ];
            out.push_str(&line.join(","));
            out.push_str("\r\n");
        }
        (out, rows.len())
    }

    /// Rebuilds the visible list if something changed. While a scan is
    /// feeding results this happens at most a few times per second.
    pub fn rebuild_if_needed(&mut self) {
        if self.dirty && (!self.scanning() || self.last_build.elapsed().as_millis() > 600) {
            self.rebuild_now();
        }
    }

    pub fn rebuild_now(&mut self) {
        self.dirty = false;
        self.last_build = Instant::now();
        let n = self.files.len();
        let nd = self.dirs.nodes.len();
        if self.folder as usize >= nd {
            self.folder = 0;
        }
        let mut in_sub = vec![false; nd];
        for d in 0..nd {
            in_sub[d] =
                d as u32 == self.folder || (d != 0 && in_sub[self.dirs.nodes[d].parent as usize]);
        }
        self.dir_total = vec![0; nd];
        self.cat_counts = [0; 6];
        self.visible.clear();
        self.passing.clear();
        self.passing.resize(n, false);
        for i in 0..n {
            let f = &self.files[i];
            if (f.status == Status::Existing && !self.show_existing)
                || !self.size_filter.pass(f.size)
                || !self.date_filter.pass(f.modified, self.now)
                || !self.chance_filter.pass(f.chance)
                || !matches_search(&self.terms, &f.name)
            {
                continue;
            }
            let dir = self.dir_of[i] as usize;
            if in_sub[dir] {
                self.cat_counts[self.cats[i] as usize] += 1;
            }
            if self
                .cat
                .map(|c| c.index() as u8 != self.cats[i])
                .unwrap_or(false)
            {
                continue;
            }
            self.passing[i] = true;
            self.dir_total[dir] += 1;
            if in_sub[dir] {
                self.visible.push(i as u32);
            }
        }
        for d in (1..nd).rev() {
            let p = self.dirs.nodes[d].parent as usize;
            self.dir_total[p] += self.dir_total[d];
        }
        self.recount_dirs();
        self.sort_visible();
        self.rebuild_tree();
        if let Some(h) = self.highlighted {
            if !self.passing.get(h as usize).copied().unwrap_or(false) {
                self.highlighted = None;
            }
        }
    }

    fn recount_dirs(&mut self) {
        let nd = self.dirs.nodes.len();
        self.dir_checked = vec![0; nd];
        for i in 0..self.files.len() {
            if self.passing[i] && self.checked[i] {
                self.dir_checked[self.dir_of[i] as usize] += 1;
            }
        }
        for d in (1..nd).rev() {
            let p = self.dirs.nodes[d].parent as usize;
            self.dir_checked[p] += self.dir_checked[d];
        }
    }

    fn sort_visible(&mut self) {
        let (col, asc) = self.sort;
        let files = &self.files;
        let dir_of = &self.dir_of;
        let paths = &self.dirs.paths;
        self.visible.sort_by(|a, b| {
            let (x, y) = (&files[*a as usize], &files[*b as usize]);
            let by_name = || cmp_ci(&x.name, &y.name);
            let o = match col {
                Col::Name => by_name(),
                Col::Size => x.size.cmp(&y.size).then_with(by_name),
                Col::Modified => x.modified.cmp(&y.modified).then_with(by_name),
                Col::Chance => chance_rank(x.chance)
                    .cmp(&chance_rank(y.chance))
                    .then_with(by_name),
                Col::Folder => {
                    let (dx, dy) = (dir_of[*a as usize], dir_of[*b as usize]);
                    if dx == dy {
                        by_name()
                    } else {
                        cmp_ci(&paths[dx as usize], &paths[dy as usize]).then_with(by_name)
                    }
                }
            };
            if asc {
                o
            } else {
                o.reverse()
            }
        });
    }

    pub fn rebuild_tree(&mut self) {
        self.tree_rows.clear();
        let mut stack: Vec<(u32, u16)> = vec![(0, 0)];
        while let Some((dir, depth)) = stack.pop() {
            if dir != 0 {
                self.tree_rows.push((dir, depth));
                if !self.expanded.contains(&dir) {
                    continue;
                }
            }
            let mut kids: Vec<u32> = self.dirs.nodes[dir as usize]
                .children
                .iter()
                .copied()
                .filter(|c| self.dir_total[*c as usize] > 0)
                .collect();
            kids.sort_by(|a, b| {
                cmp_ci(
                    &self.dirs.nodes[*b as usize].name,
                    &self.dirs.nodes[*a as usize].name,
                )
            });
            let next = if dir == 0 { 0 } else { depth + 1 };
            stack.extend(kids.into_iter().map(|k| (k, next)));
        }
    }

    pub fn has_visible_children(&self, dir: u32) -> bool {
        self.dirs.nodes[dir as usize]
            .children
            .iter()
            .any(|c| self.dir_total[*c as usize] > 0)
    }

    fn set_one(&mut self, i: usize, on: bool) {
        if self.checked[i] != on {
            self.checked[i] = on;
            let size = self.files[i].size;
            if on {
                self.sel_count += 1;
                self.sel_bytes += size;
            } else {
                self.sel_count -= 1;
                self.sel_bytes -= size;
            }
        }
    }

    pub fn toggle(&mut self, i: u32) {
        let on = !self.checked[i as usize];
        self.set_one(i as usize, on);
        self.recount_dirs();
    }

    /// Ticks or clears every file that is shown right now.
    pub fn set_visible(&mut self, on: bool) {
        for k in 0..self.visible.len() {
            let i = self.visible[k] as usize;
            self.set_one(i, on);
        }
        self.recount_dirs();
    }

    /// Ticks or clears every file (that passes the filters) in a folder and below.
    pub fn set_folder(&mut self, dir: u32, on: bool) {
        let nd = self.dirs.nodes.len();
        let mut in_sub = vec![false; nd];
        for d in 0..nd {
            in_sub[d] = d as u32 == dir || (d != 0 && in_sub[self.dirs.nodes[d].parent as usize]);
        }
        for i in 0..self.files.len() {
            if self.passing[i] && in_sub[self.dir_of[i] as usize] {
                self.set_one(i, on);
            }
        }
        self.recount_dirs();
    }

    pub fn clear_selection(&mut self) {
        self.checked.iter_mut().for_each(|c| *c = false);
        self.sel_count = 0;
        self.sel_bytes = 0;
        self.recount_dirs();
    }

    /// `Some(true)` all shown files ticked, `Some(false)` none, `None` some.
    pub fn visible_state(&self) -> Option<bool> {
        let ticked = self
            .visible
            .iter()
            .filter(|i| self.checked[**i as usize])
            .count();
        if ticked == 0 {
            Some(false)
        } else if ticked == self.visible.len() {
            Some(true)
        } else {
            None
        }
    }

    pub fn folder_state(&self, dir: u32) -> Option<bool> {
        let (c, t) = (self.dir_checked[dir as usize], self.dir_total[dir as usize]);
        if c == 0 {
            Some(false)
        } else if c == t {
            Some(true)
        } else {
            None
        }
    }

    pub fn selected_indices(&self) -> Vec<u32> {
        (0..self.files.len() as u32)
            .filter(|i| self.checked[*i as usize])
            .collect()
    }

    /// Moves the highlight up or down the visible list.
    pub fn step_highlight(&mut self, delta: i64) {
        if self.visible.is_empty() {
            return;
        }
        let pos = self
            .highlighted
            .and_then(|h| self.visible.iter().position(|v| *v == h))
            .map(|p| p as i64 + delta)
            .unwrap_or(0)
            .clamp(0, self.visible.len() as i64 - 1);
        self.highlighted = Some(self.visible[pos as usize]);
        self.scroll_to_highlight = true;
    }

    /// Moves the highlight to the first or the last file shown.
    pub fn jump_highlight(&mut self, last: bool) {
        if let Some(i) = if last {
            self.visible.last()
        } else {
            self.visible.first()
        } {
            self.highlighted = Some(*i);
            self.scroll_to_highlight = true;
        }
    }

    /// Position of the highlighted file in the list that is shown.
    pub fn highlighted_row(&self) -> Option<usize> {
        let h = self.highlighted?;
        self.visible.iter().position(|v| *v == h)
    }
}

/// A number that stands for one listed entry: same file, same number.
fn identity(f: &FoundFile) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f.name.hash(&mut h);
    f.path.hash(&mut h);
    f.size.hash(&mut h);
    f.modified.hash(&mut h);
    f.origin.hash(&mut h);
    f.first_offset().hash(&mut h);
    h.finish()
}

/// The new scroll position that brings a row into view, if it is not
/// already visible. `top`..`top + size` is the row, `offset` and `height`
/// describe what is shown now.
pub fn scroll_into_view(top: f32, size: f32, offset: f32, height: f32) -> Option<f32> {
    if height <= 0.0 {
        return None;
    }
    if top < offset {
        Some(top)
    } else if top + size > offset + height {
        Some((top + size - height).max(0.0))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Extent, FileData, Origin};

    fn file(
        name: &str,
        path: &str,
        size: u64,
        modified: i64,
        chance: Chance,
        status: Status,
    ) -> FoundFile {
        FoundFile {
            name: name.into(),
            path: path.into(),
            size,
            modified,
            created: 0,
            status,
            origin: Origin::Ntfs,
            chance,
            data: FileData::Extents(vec![Extent {
                offset: size * 512,
                len: size,
            }]),
            note: String::new(),
        }
    }

    fn session() -> Session {
        let mut s = Session::new(
            SourceSpec::Demo,
            "Test".into(),
            None,
            ScanOptions::default(),
        );
        let now = s.now;
        s.add_files(vec![
            file(
                "b.jpg",
                "Photos/Trip",
                2_000_000,
                now - 100,
                Chance::Good,
                Status::Deleted,
            ),
            file(
                "A.jpg",
                "Photos/Trip",
                500,
                now - 40 * 86_400,
                Chance::Partial,
                Status::Deleted,
            ),
            file(
                "c.png",
                "Photos",
                300,
                now - 800 * 86_400,
                Chance::Poor,
                Status::Deleted,
            ),
            file(
                "song.mp3",
                "Music",
                5_000_000,
                0,
                Chance::Good,
                Status::Deleted,
            ),
            file(
                "kept.pdf",
                "Docs",
                9_000,
                now - 5,
                Chance::Good,
                Status::Existing,
            ),
            file(
                "report.pdf",
                "Docs/Work",
                200 << 20,
                now - 5,
                Chance::Good,
                Status::Deleted,
            ),
        ]);
        s.rebuild_now();
        s
    }

    fn names(s: &Session) -> Vec<&str> {
        s.visible
            .iter()
            .map(|i| s.files[*i as usize].name.as_str())
            .collect()
    }

    #[test]
    fn filters_counts_and_sorting() {
        let mut s = session();
        // Existing files are hidden by default; sorted by folder, then name.
        assert_eq!(
            names(&s),
            ["report.pdf", "song.mp3", "c.png", "A.jpg", "b.jpg"]
        );
        assert_eq!(s.cat_counts[Category::Pictures.index()], 3);
        assert_eq!(s.cat_counts[Category::Documents.index()], 1);
        s.show_existing = true;
        s.rebuild_now();
        assert_eq!(s.visible.len(), 6);
        s.show_existing = false;

        s.cat = Some(Category::Pictures);
        s.sort = (Col::Name, true);
        s.rebuild_now();
        assert_eq!(names(&s), ["A.jpg", "b.jpg", "c.png"]);
        s.sort = (Col::Size, false);
        s.rebuild_now();
        assert_eq!(names(&s), ["b.jpg", "A.jpg", "c.png"]);
        s.sort = (Col::Chance, true);
        s.rebuild_now();
        assert_eq!(names(&s), ["b.jpg", "A.jpg", "c.png"]);
        // Chips keep showing how many files each type would give.
        assert_eq!(s.cat_counts[Category::Audio.index()], 1);

        s.cat = None;
        s.size_filter = SizeFilter::Over100Mb;
        s.rebuild_now();
        assert_eq!(names(&s), ["report.pdf"]);
        s.size_filter = SizeFilter::Under1Mb;
        s.date_filter = DateFilter::Older;
        s.rebuild_now();
        assert_eq!(names(&s), ["c.png"]);
        s.size_filter = SizeFilter::Any;
        s.date_filter = DateFilter::Last7Days;
        s.rebuild_now();
        assert_eq!(
            names(&s).len(),
            2,
            "files without a date do not match a date filter"
        );
        s.date_filter = DateFilter::Any;
        s.chance_filter = ChanceFilter::Good;
        s.sort = (Col::Name, true);
        s.rebuild_now();
        assert_eq!(names(&s), ["b.jpg", "report.pdf", "song.mp3"]);
        s.chance_filter = ChanceFilter::Any;
        s.set_search("  JPG ");
        assert_eq!(names(&s), ["A.jpg", "b.jpg"]);
        s.set_search("");
        assert_eq!(s.visible.len(), 5);
    }

    #[test]
    fn folders_and_selection_totals() {
        let mut s = session();
        let photos = s.dirs.intern("Photos");
        let trip = s.dirs.intern("Photos/Trip");
        assert_eq!(s.dir_total[photos as usize], 3);
        assert_eq!(s.dir_total[0], 5);
        // Top-level folders are listed; the first two levels start opened.
        let listed: Vec<&str> = s
            .tree_rows
            .iter()
            .map(|(d, _)| s.dirs.paths[*d as usize].as_str())
            .collect();
        assert_eq!(
            listed,
            ["Docs", "Docs/Work", "Music", "Photos", "Photos/Trip"]
        );
        assert!(!s.has_visible_children(trip));

        s.folder = photos;
        s.rebuild_now();
        assert_eq!(s.visible.len(), 3);
        assert_eq!(s.cat_counts[Category::Audio.index()], 0);

        s.set_folder(trip, true);
        assert_eq!((s.sel_count, s.sel_bytes), (2, 2_000_500));
        assert_eq!(s.folder_state(trip), Some(true));
        assert_eq!(s.folder_state(photos), None);
        assert_eq!(s.folder_state(0), None);
        assert_eq!(s.visible_state(), None);
        s.set_visible(true);
        assert_eq!(s.sel_count, 3);
        assert_eq!(s.visible_state(), Some(true));
        let c = s.visible[0];
        s.toggle(c);
        assert_eq!(s.sel_count, 2);
        s.folder = 0;
        s.rebuild_now();
        s.set_visible(true);
        assert_eq!(
            (s.sel_count, s.sel_bytes),
            (5, 2_000_800 + 5_000_000 + (200 << 20))
        );
        assert_eq!(s.selected_indices().len(), 5);
        s.clear_selection();
        assert_eq!((s.sel_count, s.sel_bytes), (0, 0));
        assert_eq!(s.folder_state(0), Some(false));

        s.step_highlight(1);
        assert_eq!(s.highlighted, Some(s.visible[0]));
        s.step_highlight(1);
        assert_eq!(s.highlighted, Some(s.visible[1]));
        s.step_highlight(-9);
        assert_eq!(s.highlighted, Some(s.visible[0]));
    }

    #[test]
    fn retracting_keeps_everything_in_step() {
        let mut s = session();
        let mut carved = file(
            "recovered-000000001.jpg",
            "Found by content/Pictures/JPG",
            700,
            0,
            Chance::Good,
            Status::Deleted,
        );
        carved.origin = Origin::Carved;
        let offset = carved.first_offset().unwrap();
        s.add_files(vec![carved]);
        s.rebuild_now();
        s.set_visible(true);
        assert_eq!(s.sel_count, 6);
        s.retract(&HashSet::from([offset]));
        assert_eq!(s.files.len(), 6);
        assert_eq!(s.checked.len(), 6);
        assert_eq!(s.dir_of.len(), 6);
        assert_eq!(s.sel_count, 5);
        assert_eq!(s.visible.len(), 5);
        assert!(!names(&s).iter().any(|n| n.starts_with("recovered")));
    }

    #[test]
    fn search_words_and_patterns() {
        let mut s = session();
        s.set_search("*.jpg");
        assert_eq!(names(&s), ["A.jpg", "b.jpg"]);
        s.set_search("*.JP? b");
        assert_eq!(names(&s), ["b.jpg"]);
        s.set_search("re pdf");
        assert_eq!(names(&s), ["report.pdf"], "every word has to match");
        s.set_search("?.png");
        assert_eq!(names(&s), ["c.png"]);
        s.set_search("*");
        assert_eq!(s.visible.len(), 5);
        s.set_search("song.mp3 nothing");
        assert!(s.visible.is_empty());
        let p = |t: &str| t.chars().collect::<Vec<char>>();
        assert!(glob(&p("a*b*c"), &p("aXXbYYc")));
        assert!(glob(&p("*"), &p("")));
        assert!(!glob(&p("a*b"), &p("ab c")));
        assert!(!glob(&p("?"), &p("")));
        assert!(glob(&p("ڕاپۆرت*.pdf"), &p("ڕاپۆرتی ساڵانە.pdf")));
    }

    #[test]
    fn clearing_filters_and_ticking_by_type() {
        let mut s = session();
        assert!(!s.filters_active());
        s.cat = Some(Category::Audio);
        s.size_filter = SizeFilter::Under1Mb;
        s.rebuild_now();
        assert!(s.filters_active());
        assert!(s.visible.is_empty());
        s.clear_filters();
        assert!(!s.filters_active());
        assert_eq!(s.visible.len(), 5);
        s.set_ext("jpg", true);
        assert_eq!((s.sel_count, s.sel_bytes), (2, 2_000_500));
        s.set_ext("jpg", false);
        assert_eq!(s.sel_count, 0);
    }

    #[test]
    fn csv_lists_the_ticked_files_or_everything_shown() {
        let mut s = session();
        let mut odd = file(
            "=cmd, \"x\".txt",
            "Docs",
            7,
            0,
            Chance::Good,
            Status::Deleted,
        );
        odd.note = "two\nlines".into();
        s.add_files(vec![odd]);
        s.rebuild_now();
        let (all, n) = s.csv(0);
        assert_eq!(n, 6);
        assert!(all.starts_with("\u{feff}Name,Folder,Size in bytes,"));
        assert_eq!(all.matches("\r\n").count(), 7);
        // Quoted, quotes doubled, and no formula for a spreadsheet to run.
        assert!(all.contains(
            "\"'=cmd, \"\"x\"\".txt\",Docs,7,,,Deleted,Good,NTFS file record,\"two\nlines\"\r\n"
        ));
        assert!(all.contains("report.pdf,Docs/Work,209715200,"));
        let i = s.visible[0];
        s.toggle(i);
        let (some, n) = s.csv(0);
        assert_eq!(n, 1);
        assert_eq!(some.matches("\r\n").count(), 2);
    }

    #[test]
    fn a_continued_scan_does_not_list_entries_twice() {
        let mut s = session();
        s.finished = true;
        s.stopped = true;
        let before = s.files.len();
        s.set_visible(true);
        let ticked = s.sel_count;
        s.begin_resume();
        assert!(!s.finished && !s.stopped);
        // The continued scan reads the file table again...
        let again: Vec<FoundFile> = s.files.clone();
        s.add_files(again);
        assert_eq!(s.files.len(), before);
        // ...and finds new things further on.
        s.add_files(vec![file(
            "recovered-000000900.jpg",
            "Found by content/Pictures/JPG",
            900,
            0,
            Chance::Good,
            Status::Deleted,
        )]);
        assert_eq!(s.files.len(), before + 1);
        assert_eq!(s.sel_count, ticked, "ticks are kept");
    }

    #[test]
    fn keyboard_moves_bring_rows_into_view() {
        let mut s = session();
        assert_eq!(s.highlighted_row(), None);
        s.jump_highlight(true);
        assert_eq!(s.highlighted_row(), Some(s.visible.len() - 1));
        assert!(s.scroll_to_highlight);
        s.jump_highlight(false);
        assert_eq!(s.highlighted_row(), Some(0));
        s.step_highlight(3);
        assert_eq!(s.highlighted_row(), Some(3));
        // Row 10 of 36-point rows in a 300-point view scrolled to 0.
        assert_eq!(scroll_into_view(360.0, 36.0, 0.0, 300.0), Some(96.0));
        assert_eq!(scroll_into_view(36.0, 36.0, 100.0, 300.0), Some(36.0));
        assert_eq!(scroll_into_view(150.0, 36.0, 100.0, 300.0), None);
        assert_eq!(scroll_into_view(150.0, 36.0, 100.0, 0.0), None);
    }

    #[test]
    fn meter_and_helpers() {
        let mut m = Meter::default();
        assert!(m.speed().is_none());
        let t = Instant::now();
        m.samples.push_back((t, 0));
        m.samples
            .push_back((t + std::time::Duration::from_secs(2), 200 << 20));
        let speed = m.speed().unwrap();
        assert!((speed - (100u64 << 20) as f64).abs() < 1.0);
        assert_eq!(m.eta(300 << 20), Some(3));
        assert_eq!(cmp_ci("abc", "ABD"), Ordering::Less);
        assert_eq!(cmp_ci("b", "A"), Ordering::Greater);
        assert_eq!(cmp_ci("a", "ab"), Ordering::Less);
        let mut d = Dirs::default();
        let deep = d.intern("a/b/c");
        assert_eq!(d.paths[deep as usize], "a/b/c");
        assert_eq!(d.nodes.len(), 4);
        assert_eq!(d.intern("a/b"), d.nodes[deep as usize].parent);
        assert_eq!(d.intern(""), 0);
    }
}
