//! The engine's places: moving folders to and from their destinations as
//! the machine's wishes change, pausing every folder whose destination is
//! missing or not yet linked, so that nothing under it is published or
//! written, and watching the destinations.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use pigeon_core::path::GroupPath;
use pigeon_core::places::Place;
use pigeon_store::config::Config;
use pigeon_store::disk::fs_path;
use pigeon_store::layout;
use serde::Serialize;

use crate::engine::{Engine, Inner, Work};
use crate::watch::{Rescan, Watched, watch};

/// A folder this machine keeps at a destination, and why it is paused.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PlaceView {
    pub folder: GroupPath,
    pub destination: PathBuf,
    /// Why the folder waits, `None` once it is at its destination.
    pub problem: Option<String>,
}

/// `destination` as the system resolves it, though it need not exist yet.
fn resolve(destination: &Path) -> Result<PathBuf> {
    if !destination.is_absolute() {
        bail!("{} is not an absolute path", destination.display());
    }
    if let Ok(resolved) = destination.canonicalize() {
        return Ok(resolved);
    }
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent folder", destination.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| anyhow!("{} names no folder", destination.display()))?;
    let parent = parent
        .canonicalize()
        .with_context(|| format!("{} is not there", parent.display()))?;
    Ok(parent.join(name))
}

impl Inner {
    /// The root as the system resolves it.
    fn resolved_root(&self) -> PathBuf {
        self.root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone())
    }

    /// Moves back the folders no longer wanted elsewhere, then moves the
    /// wanted ones to their destinations, as far as each destination
    /// allows; the folders left out of place pause, and the watcher
    /// follows the destinations reached.
    pub(crate) fn lay_out(&self, work: &mut Work) {
        let root = &self.root;
        let mut problems: Vec<(GroupPath, String)> = Vec::new();
        let placed: Vec<Place> = work.placed.iter().collect();
        for place in placed {
            if work.config.places.get(&place.folder) == Some(place.destination.as_path()) {
                continue;
            }
            match layout::unplace(&fs_path(root, &place.folder), &place.destination) {
                Ok(()) => {
                    let _ = work.placed.remove(&place.folder);
                    self.save_placed(work);
                }
                Err(error) => problems.push((place.folder, error.to_string())),
            }
        }
        let wanted: Vec<Place> = work.config.places.iter().collect();
        for place in wanted {
            let location = fs_path(root, &place.folder);
            let applied = work.placed.get(&place.folder) == Some(place.destination.as_path());
            if applied && layout::is_in_place(&location, &place.destination) {
                continue;
            }
            match self.place_one(work, &location, &place, applied) {
                Ok(()) => self.save_placed(work),
                Err(problem) => problems.push((place.folder, problem)),
            }
        }
        if problems != work.out_of_place {
            for (folder, problem) in &problems {
                self.report(format!("{folder} waits for its destination: {problem}"));
            }
            work.out_of_place = problems;
        }
        self.follow_destinations(work);
    }

    /// Moves one wanted folder to its destination; a destination that went
    /// missing after the move is never made again, since the files are
    /// there.
    fn place_one(
        &self,
        work: &mut Work,
        location: &Path,
        place: &Place,
        applied: bool,
    ) -> Result<(), String> {
        if applied && !place.destination.is_dir() {
            return Err(format!(
                "{} is not there: is its disk plugged in?",
                place.destination.display()
            ));
        }
        let mut placed = work.placed.clone();
        placed
            .set(
                &self.resolved_root(),
                place.folder.clone(),
                place.destination.clone(),
            )
            .map_err(|error| format!("{error}, which has not moved back yet"))?;
        layout::place(location, &place.destination).map_err(|error| error.to_string())?;
        work.placed = placed;
        Ok(())
    }

    fn save_placed(&self, work: &Work) {
        if let Err(error) = self.state.set_placed(&work.placed) {
            self.report(error);
        }
    }

    /// Watches the root and every destination reached, when they changed.
    fn follow_destinations(&self, work: &mut Work) {
        let watched = Watched::all(&self.root, &work.in_place());
        if watched == work.watched && work.watcher.is_some() {
            return;
        }
        match watch(watched.clone(), self.rescans.clone()) {
            Ok(started) => {
                work.watcher = Some(started);
                work.watched = watched;
            }
            Err(error) => self.report(format!("watching the root: {error}")),
        }
    }
}

impl Work {
    /// The placed folders at their destinations.
    pub(crate) fn in_place(&self) -> Vec<Place> {
        self.placed
            .iter()
            .filter(|place| !self.is_out_of_place(&place.folder))
            .collect()
    }

    /// Whether `path` lies in a folder out of place.
    pub(crate) fn is_out_of_place(&self, path: &GroupPath) -> bool {
        let key = path.key();
        self.out_of_place
            .iter()
            .any(|(folder, _)| key.is_within(&folder.key()))
    }

    /// Why `folder` is out of place, if it is.
    fn problem(&self, folder: &GroupPath) -> Option<&str> {
        self.out_of_place
            .iter()
            .find(|(paused, _)| paused.key() == folder.key())
            .map(|(_, problem)| problem.as_str())
    }
}

impl Engine {
    /// Keeps `folder` at `destination`, an absolute path whose parent
    /// folder exists, moving what this machine holds of it there and
    /// leaving a link at its place.
    ///
    /// # Errors
    ///
    /// Fails if the folder or the destination nests with the root or
    /// another place, or the move did not finish; the folder then stays
    /// paused until it does, which pigeon retries at every rescan.
    pub async fn place(&self, folder: GroupPath, destination: &Path) -> Result<()> {
        let inner = &self.inner;
        let destination = resolve(destination)?;
        let mut work = inner.work.lock().await;
        let mut config = Config::clone(&work.config);
        config
            .places
            .set(&inner.resolved_root(), folder.clone(), destination)?;
        work.config.save(config)?;
        self.settle_layout(&mut work, &folder).await
    }

    /// Brings `folder` back from its destination into the root.
    ///
    /// # Errors
    ///
    /// Fails if the folder has no destination, or the move back did not
    /// finish; the folder then stays paused until it does.
    pub async fn unplace(&self, folder: &GroupPath) -> Result<()> {
        let inner = &self.inner;
        let mut work = inner.work.lock().await;
        let mut config = Config::clone(&work.config);
        if !config.places.remove(folder) {
            bail!("{folder} has no destination: see `pigeon selection places`");
        }
        work.config.save(config)?;
        self.settle_layout(&mut work, folder).await
    }

    async fn settle_layout(&self, work: &mut Work, folder: &GroupPath) -> Result<()> {
        let inner = &self.inner;
        inner.lay_out(work);
        inner.refresh(work, &Rescan::All).await;
        match work.problem(folder) {
            Some(problem) => bail!("{folder} waits: {problem}"),
            None => Ok(()),
        }
    }

    /// The folders this machine keeps elsewhere, wanted or still moving.
    pub async fn places(&self) -> Vec<PlaceView> {
        let work = self.inner.work.lock().await;
        let mut views: Vec<PlaceView> = work
            .config
            .places
            .iter()
            .chain(
                work.placed
                    .iter()
                    .filter(|place| work.config.places.get(&place.folder).is_none()),
            )
            .map(|place| PlaceView {
                problem: work.problem(&place.folder).map(str::to_owned),
                folder: place.folder,
                destination: place.destination,
            })
            .collect();
        views.sort_by(|a, b| a.folder.cmp(&b.folder));
        views
    }
}
