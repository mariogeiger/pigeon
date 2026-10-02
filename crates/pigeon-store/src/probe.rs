//! What the disk holds at a group path, as pigeon sees it: each folder
//! listed once, a name matched whatever its case, the smallest portable
//! spelling winning among the names of one key, no link followed but those
//! of placed folders, and what `.pigeonignore` files exclude out of sight.
//! A file is absent only where a folder that could be read lists none. A
//! name no group path holds is kept out, with the portable name closest to
//! it that its folder leaves free.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, FileType};
use std::path::{Path, PathBuf};

use pigeon_core::path::{GroupPath, PathError, PathKey, check_disk_name, name_key, portable_name};

use crate::disk::{Stat, TEMPORARY_PREFIX};
use crate::ignore_rules::IgnoreRules;

/// A file found on disk under a portable name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    /// The path as the disk spells it.
    pub path: GroupPath,
    pub location: PathBuf,
    pub stat: Stat,
}

/// What the disk holds at one group path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Probe {
    /// A regular file.
    Present(Found),
    /// No regular file, as a folder that could be read shows.
    Absent,
    /// Nothing can be told, for this reason.
    Unknown(String),
    /// An ignore file keeps the path out of sight.
    Ignored,
}

/// A name on disk that no group path holds as it is spelled, which pigeon
/// keeps out of the group.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Unportable {
    /// The folder that holds it, as the disk spells it; `None` for the
    /// root.
    pub folder: Option<GroupPath>,
    /// The name as the disk spells it, any invalid Unicode replaced.
    pub name: String,
    pub location: PathBuf,
    pub reason: String,
    /// The portable name closest to it that no other name of the folder
    /// takes, whatever its case.
    pub proposal: String,
}

impl Unportable {
    /// Where it lies in the group: its folder's path, then its name.
    #[must_use]
    pub fn path(&self) -> String {
        match &self.folder {
            Some(folder) => format!("{folder}/{}", self.name),
            None => self.name.clone(),
        }
    }

    /// The path it takes once renamed to the proposal.
    ///
    /// # Errors
    /// Returns the rule the path breaks, which no proposal does.
    pub fn proposed(&self) -> Result<GroupPath, PathError> {
        let folder = self.folder.iter().flat_map(GroupPath::names);
        GroupPath::from_names(folder.chain([self.proposal.as_str()]))
    }

    /// Whether a scan of `under` sees it: it lies in that folder, or is
    /// named as `under`, whatever the case and spelling, in its folder.
    #[must_use]
    pub fn lies_within(&self, under: &GroupPath) -> bool {
        let folder = self.folder.as_ref().map(GroupPath::key);
        let names: Vec<&str> = under.names().collect();
        let parent = GroupPath::from_names(names[..names.len() - 1].iter().copied()).ok();
        folder
            .as_ref()
            .is_some_and(|folder| folder.is_within(&under.key()))
            || (folder == parent.as_ref().map(GroupPath::key)
                && name_key(&self.name) == name_key(under.file_name()))
    }

    /// This name, of the folder at `spelled`.
    pub(crate) fn in_folder(&self, spelled: &[String]) -> Self {
        Self {
            folder: GroupPath::from_names(spelled.iter().map(String::as_str)).ok(),
            ..self.clone()
        }
    }
}

/// An entry of a folder under a portable name.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub name: String,
    /// The name as the disk spells it.
    pub written: String,
    pub kind: FileType,
}

/// A folder as pigeon sees it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Listing {
    /// The entries in sight, by caseless name.
    pub entries: BTreeMap<String, Entry>,
    pub unportable: Vec<Unportable>,
    /// The files pigeon was writing when it stopped.
    pub temporaries: Vec<PathBuf>,
}

/// The disk of one root as one look sees it: every folder is listed at
/// most once, so that every answer agrees with every other.
#[derive(Debug)]
pub struct Prober {
    pub(crate) root: PathBuf,
    pub(crate) placed: BTreeSet<PathKey>,
    pub(crate) listings: HashMap<PathBuf, Result<Listing, String>>,
    pub(crate) rules: IgnoreRules,
}

/// Where a descent through the folders of a path stopped short.
pub(crate) enum Stop {
    /// Where the descent went, the folders' names as the disk spells them
    /// and the next name, missing.
    Missing(PathBuf, Vec<String>),
    /// What the path holds, said already: absent, unknown or ignored.
    Told(Probe),
}

impl Prober {
    /// A look at `root`, whose `placed` folders lead to their destinations.
    #[must_use]
    pub fn new(root: &Path, placed: &[GroupPath]) -> Self {
        Self {
            root: root.to_path_buf(),
            placed: placed.iter().map(GroupPath::key).collect(),
            listings: HashMap::new(),
            rules: IgnoreRules::new(root),
        }
    }

    /// What the disk holds at `path`.
    pub fn probe(&mut self, path: &GroupPath) -> Probe {
        let names: Vec<&str> = path.names().collect();
        let Some((last, folders)) = names.split_last() else {
            return Probe::Absent;
        };
        let (folder, mut spelled) = match self.descend(folders) {
            Ok(reached) => reached,
            Err(Stop::Told(probe)) => return probe,
            Err(Stop::Missing(..)) => return Probe::Absent,
        };
        match self.find(&folder, last, false) {
            Ok(Some(entry)) => {
                spelled.push(entry.name.clone());
                file_at(&spelled, &folder.join(&entry.written), &entry)
            }
            Ok(None) => Probe::Absent,
            Err(probe) => probe,
        }
    }

    /// Where a file written at `path` goes, with the path as the disk then
    /// spells it: into the folders the disk holds under any spelling of
    /// their names, then as `path` spells the rest; and the folder in
    /// which the write makes a folder, if it does, whose listing it
    /// changes.
    pub fn locate(&mut self, path: &GroupPath) -> (GroupPath, PathBuf, Option<PathBuf>) {
        let names: Vec<&str> = path.names().collect();
        let folders = &names[..names.len() - 1];
        let (mut location, mut spelled, rest, grows) = match self.descend(folders) {
            Ok((folder, spelled)) => (folder, spelled, &[][..], None),
            Err(Stop::Missing(folder, spelled)) => {
                let rest = &folders[spelled.len()..];
                (folder.clone(), spelled, rest, Some(folder))
            }
            Err(Stop::Told(_)) => {
                return (path.clone(), crate::disk::fs_path(&self.root, path), None);
            }
        };
        for name in rest.iter().chain([&path.file_name()]) {
            location.push(name);
            spelled.push((*name).to_owned());
        }
        let spelled = GroupPath::from_names(spelled.iter().map(String::as_str))
            .unwrap_or_else(|_| path.clone());
        (spelled, location, grows)
    }

    /// The name `name` of the folder `folder`, or of the root, if it is
    /// still one no group path holds, as the disk shows it now.
    pub fn unportable_at(&mut self, folder: Option<&GroupPath>, name: &str) -> Option<Unportable> {
        let names: Vec<&str> = folder.iter().flat_map(|folder| folder.names()).collect();
        let (location, spelled) = self.descend(&names).ok()?;
        let listing = self.listing(&location).ok()?;
        listing
            .unportable
            .iter()
            .find(|unportable| unportable.name == name)
            .map(|unportable| unportable.in_folder(&spelled))
    }

    /// Forgets what `folder` listed, which changed.
    pub fn forget(&mut self, folder: &Path) {
        self.listings.remove(folder);
    }

    /// Goes down the folders `names` from the root, matching each name
    /// whatever its case and following the links of placed folders: where
    /// it reached, with the names as the disk spells them.
    pub(crate) fn descend(&mut self, names: &[&str]) -> Result<(PathBuf, Vec<String>), Stop> {
        let mut folder = self.root.clone();
        let mut spelled = Vec::new();
        for name in names {
            let Some(entry) = self.find(&folder, name, true).map_err(Stop::Told)? else {
                return Err(Stop::Missing(folder, spelled));
            };
            spelled.push(entry.name.clone());
            if !leads_into(&self.placed, &spelled, &entry) {
                return Err(Stop::Told(Probe::Absent));
            }
            folder.push(&entry.written);
        }
        Ok((folder, spelled))
    }

    /// The entry of `folder` named `name` whatever its case, none when the
    /// folder lists no such name; or what the path holds when the folder
    /// cannot be read or the name, a folder's when `is_folder`, is
    /// ignored.
    pub(crate) fn find(
        &mut self,
        folder: &Path,
        name: &str,
        is_folder: bool,
    ) -> Result<Option<Entry>, Probe> {
        let found = match self.listing(folder) {
            Ok(listing) => listing.entries.get(&name.to_lowercase()).cloned(),
            Err(error) => return Err(Probe::Unknown(error.clone())),
        };
        if found.is_none() && self.rules.excludes(&folder.join(name), is_folder) {
            return Err(Probe::Ignored);
        }
        Ok(found)
    }

    pub(crate) fn listing(&mut self, folder: &Path) -> Result<&Listing, &String> {
        listing(&mut self.listings, &mut self.rules, folder)
    }
}

/// `folder` as pigeon sees it, listed into `listings` on the first call.
pub(crate) fn listing<'a>(
    listings: &'a mut HashMap<PathBuf, Result<Listing, String>>,
    rules: &mut IgnoreRules,
    folder: &Path,
) -> Result<&'a Listing, &'a String> {
    listings
        .entry(folder.to_path_buf())
        .or_insert_with(|| list(folder, rules))
        .as_ref()
}

/// Whether `entry`, at `spelled`, is a folder pigeon goes into: a folder,
/// or the link of one of the `placed` folders.
pub(crate) fn leads_into(placed: &BTreeSet<PathKey>, spelled: &[String], entry: &Entry) -> bool {
    entry.kind.is_dir()
        || (entry.kind.is_symlink()
            && GroupPath::from_names(spelled.iter().map(String::as_str))
                .is_ok_and(|path| placed.contains(&path.key())))
}

/// What `entry` of a folder's listing, at `spelled` and `location`, holds
/// now: a regular file, or none.
pub(crate) fn file_at(spelled: &[String], location: &Path, entry: &Entry) -> Probe {
    if !entry.kind.is_file() {
        return Probe::Absent;
    }
    let path = match GroupPath::from_names(spelled.iter().map(String::as_str)) {
        Ok(path) => path,
        Err(error) => return Probe::Unknown(error.to_string()),
    };
    match fs::symlink_metadata(location) {
        Ok(metadata) if metadata.is_file() => Probe::Present(Found {
            path,
            location: location.to_path_buf(),
            stat: Stat::of(&metadata),
        }),
        Ok(_) => Probe::Absent,
        Err(error) => Probe::Unknown(format!("{}: {error}", location.display())),
    }
}

/// Lists `folder`: each name in sight under its portable spelling, the
/// smallest spelling of each caseless name, and the names that are no
/// portable spelling, with why and a free portable name for each.
fn list(folder: &Path, rules: &mut IgnoreRules) -> Result<Listing, String> {
    let unreadable =
        |error: std::io::Error| format!("{} cannot be read: {error}", folder.display());
    let mut listing = Listing::default();
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    let mut out: Vec<(PathBuf, String, String)> = Vec::new();
    let mut spellings: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for entry in fs::read_dir(folder).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let kind = entry.file_type().map_err(unreadable)?;
        let location = entry.path();
        let lossy = entry.file_name().to_string_lossy().into_owned();
        *names.entry(name_key(&lossy)).or_default() += 1;
        if lossy.starts_with(TEMPORARY_PREFIX) {
            listing.temporaries.push(location);
            continue;
        }
        let is_folder = kind.is_dir() || (kind.is_symlink() && location.is_dir());
        if rules.excludes(&location, is_folder) {
            continue;
        }
        let Some(written) = entry.file_name().to_str().map(str::to_owned) else {
            out.push((location, lossy, "the name is not valid Unicode".to_owned()));
            continue;
        };
        let name = match check_disk_name(&written) {
            Ok(()) => written.clone(),
            Err(error) => {
                if let PathError::Unnormalized(_) = error
                    && let Some(name) = normalized_alias(&written, &location)
                {
                    name
                } else {
                    out.push((location, written, error.to_string()));
                    continue;
                }
            }
        };
        let entry = Entry {
            name,
            written,
            kind,
        };
        spellings
            .entry(entry.name.to_lowercase())
            .or_default()
            .push(entry);
    }
    for (caseless, mut entries) in spellings {
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let mut entries = entries.into_iter();
        let first = entries.next().expect("a caseless name has a spelling");
        for other in entries {
            let reason = format!("{} differs only by case from {}", other.name, first.name);
            out.push((folder.join(&other.written), other.written, reason));
        }
        listing.entries.insert(caseless, first);
    }
    listing.unportable = out
        .into_iter()
        .map(|(location, name, reason)| Unportable {
            folder: None,
            proposal: free_name(&name, &mut names),
            name,
            location,
            reason,
        })
        .collect();
    Ok(listing)
}

/// The portable name closest to `name` that no other of the folder's
/// `names`, counted by key, takes, numbered when it must be; it takes it.
fn free_name(name: &str, names: &mut BTreeMap<String, usize>) -> String {
    let own = name_key(name);
    let base = portable_name(name);
    let mut proposal = base.clone();
    for number in 2.. {
        let key = name_key(&proposal);
        let taken = names.get(&key).copied().unwrap_or(0);
        if taken == usize::from(key == own) {
            *names.entry(key).or_default() += 1;
            break;
        }
        proposal = numbered(&base, number);
    }
    proposal
}

/// `name` with `number` in brackets before its extension.
fn numbered(name: &str, number: usize) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({number}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({number})"),
    }
}

/// The NFC spelling of `written`, the name of the file at `location`,
/// where the system takes it for the same file, as systems that normalize
/// names do.
fn normalized_alias(written: &str, location: &Path) -> Option<String> {
    let name = GroupPath::parse(written).ok()?.as_str().to_owned();
    let inode = |path: &Path| Stat::read(path).ok()?.inode;
    let same = inode(location)?;
    (inode(&location.with_file_name(&name)) == Some(same)).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ignore_rules::IGNORE_FILE;

    fn write(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, relative).unwrap();
    }

    fn path(text: &str) -> GroupPath {
        GroupPath::parse(text).unwrap()
    }

    fn found_at(probe: Probe) -> String {
        match probe {
            Probe::Present(found) => found.path.to_string(),
            other => panic!("no file: {other:?}"),
        }
    }

    #[test]
    fn finds_a_file_whatever_the_case_spelled_as_the_disk_spells_it() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Docs/README.md");
        let mut prober = Prober::new(dir.path(), &[]);
        assert_eq!(
            found_at(prober.probe(&path("docs/readme.md"))),
            "Docs/README.md"
        );
        assert_eq!(prober.probe(&path("docs/other.md")), Probe::Absent);
        assert_eq!(prober.probe(&path("none/other.md")), Probe::Absent);
    }

    #[test]
    fn something_else_than_a_file_is_no_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "file");
        write(dir.path(), "folder/inside");
        let mut prober = Prober::new(dir.path(), &[]);
        assert_eq!(prober.probe(&path("file/below")), Probe::Absent);
        assert_eq!(prober.probe(&path("folder")), Probe::Absent);
    }

    #[test]
    fn an_ignored_path_is_ignored_whether_or_not_a_file_is_there() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "build/out");
        write(dir.path(), "notes.log");
        fs::write(dir.path().join(IGNORE_FILE), "build/\n*.log\n").unwrap();
        let mut prober = Prober::new(dir.path(), &[]);
        assert_eq!(prober.probe(&path("build/out")), Probe::Ignored);
        assert_eq!(prober.probe(&path("notes.log")), Probe::Ignored);
        assert_eq!(prober.probe(&path("gone.log")), Probe::Ignored);
        assert_eq!(prober.probe(&path("gone.txt")), Probe::Absent);
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_told_under_a_folder_that_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "closed/file");
        let closed = dir.path().join("closed");
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&closed).is_ok();
        let probe = Prober::new(dir.path(), &[]).probe(&path("closed/file"));
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert!(matches!(probe, Probe::Unknown(_)), "{probe:?}");
        }
        let missing = dir.path().join("missing");
        let probe = Prober::new(&missing, &[]).probe(&path("file"));
        assert!(matches!(probe, Probe::Unknown(_)), "{probe:?}");
    }

    #[cfg(unix)]
    #[test]
    fn only_the_link_of_a_placed_folder_leads_on_and_nothing_is_told_when_it_dangles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        write(dir.path(), "disk/videos/v.mp4");
        fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(dir.path().join("disk/videos"), root.join("videos")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), root.join("unplugged")).unwrap();
        let placed = [path("videos"), path("unplugged")];
        let mut prober = Prober::new(&root, &placed);
        assert_eq!(
            found_at(prober.probe(&path("videos/v.mp4"))),
            "videos/v.mp4"
        );
        let dangling = prober.probe(&path("unplugged/v.mp4"));
        assert!(matches!(dangling, Probe::Unknown(_)), "{dangling:?}");
        let mut unplaced = Prober::new(&root, &[]);
        assert_eq!(unplaced.probe(&path("videos/v.mp4")), Probe::Absent);
    }

    #[cfg(unix)]
    #[test]
    fn a_name_spelled_otherwise_than_in_nfc_is_kept_out() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "caf\u{65}\u{301}.txt");
        let mut prober = Prober::new(dir.path(), &[]);
        let probe = prober.probe(&path("caf\u{e9}.txt"));
        let distinct = !dir.path().join("caf\u{e9}.txt").exists();
        if distinct {
            assert_eq!(probe, Probe::Absent);
            let scan = prober.scan(None);
            assert!(scan.files.is_empty());
            assert_eq!(scan.unportable.len(), 1);
        } else {
            assert_eq!(found_at(probe), "caf\u{e9}.txt");
        }
    }

    #[test]
    fn a_write_goes_into_the_folders_the_disk_holds_whatever_their_case() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Docs/a");
        let mut prober = Prober::new(dir.path(), &[]);
        let (spelled, location, grows) = prober.locate(&path("docs/new/b"));
        assert_eq!(spelled.as_str(), "Docs/new/b");
        assert_eq!(location, dir.path().join("Docs/new/b"));
        assert_eq!(grows, Some(dir.path().join("Docs")));
        let (spelled, _, grows) = prober.locate(&path("DOCS/c"));
        assert_eq!(spelled.as_str(), "Docs/c");
        assert_eq!(grows, None);
    }
}
