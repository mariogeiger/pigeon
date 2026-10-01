//! The engine of one group on one machine: it opens the group's state,
//! binds its endpoint, and runs the one loop that receives patches, follows
//! the root's changes, publishes settled edits, joins the member to the
//! group, renews and keeps the group secret, and keeps the blobs it needs
//! from garbage collection.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use iroh::address_lookup::MemoryLookup;
use iroh_base::SecretKey;
use iroh_blobs::api::TempTag;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use pigeon_core::clock::{Clock, MachineId, Stamp, ntp_time};
use pigeon_core::identity::{GroupId, MachineCert, Renewal, RenewedSecret};
use pigeon_core::ledger::Ledger;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Change, Content, ContentHash, Patch, SignedPatch};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::{Cutoff, Rule, Selection};
use pigeon_core::statement::{MemberStatement, STATEMENTS, member_path};
use pigeon_net::bind::{bind_internet, bind_local};
use pigeon_net::wire::{Patches, Vector};
use pigeon_net::{Log, Node, Received};
use pigeon_store::blobs::Blobs;
use pigeon_store::config::{DataDir, GroupConfig};
use pigeon_store::disk::Stat;
use pigeon_store::group_key::random_secret;
use pigeon_store::state::State;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::watch::{Rescan, watch};

/// How the endpoint finds other machines.
#[derive(Clone, Debug)]
pub enum Network {
    /// Relays, address lookup, and discovery on the local network.
    Internet,
    /// Only the machines added to this lookup, on this host.
    Local(MemoryLookup),
}

/// The engine's timings.
#[derive(Clone, Debug)]
pub struct Options {
    pub network: Network,
    /// How long an edit in a personal folder must stay unchanged before it
    /// is published.
    pub settle_personal: Duration,
    /// The same for a draft in a drop folder, which freezes once published.
    pub settle_drop: Duration,
    /// How often the whole root is compared with the ledger.
    pub rescan: Duration,
    /// How often settled edits are published.
    pub tick: Duration,
    /// How long a new machine listens before claiming its member name.
    pub join_delay: Duration,
    /// How often garbage collection runs on the blob store.
    pub gc: Duration,
    /// How far ahead of this machine's clock a patch may be dated.
    pub max_drift: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            network: Network::Internet,
            settle_personal: Duration::from_secs(3),
            settle_drop: Duration::from_secs(300),
            rescan: Duration::from_secs(600),
            tick: Duration::from_secs(1),
            join_delay: Duration::from_secs(5),
            gc: Duration::from_secs(3600),
            max_drift: Duration::from_secs(300),
        }
    }
}

/// Whether the member belongs to the group, as this machine's certificate
/// says.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase", tag = "state", content = "reason")]
pub enum JoinState {
    /// The member file is not accepted yet.
    Pending,
    Joined,
    /// Another password holds the name, or a folder claims it.
    Taken(String),
    /// The name was bound to a new password, which logs this machine in.
    Rebound(String),
    /// The member left or was excluded.
    Excluded(String),
}

impl JoinState {
    /// Whether the machine keeps its root in agreement with the ledger.
    #[must_use]
    pub fn syncs(&self) -> bool {
        matches!(self, Self::Pending | Self::Joined)
    }
}

/// The ledger, shared with the network sessions.
pub(crate) struct SharedLedger(Mutex<Ledger>);

impl SharedLedger {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Ledger> {
        self.0.lock().expect("no panic holds the ledger")
    }
}

impl Log for SharedLedger {
    fn vector(&self) -> Vector {
        self.lock().vector()
    }

    fn missing_from(&self, vector: &Vector) -> Patches {
        self.lock()
            .missing_from(vector)
            .into_iter()
            .cloned()
            .collect()
    }

    fn recognizes(&self, cert: &MachineCert) -> bool {
        self.lock().recognizes(cert)
    }

    fn last_exclusion(&self) -> Option<Stamp> {
        self.lock().last_exclusion()
    }
}

/// An edit waiting to settle.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub path: GroupPath,
    pub location: std::path::PathBuf,
    pub stat: Option<Stat>,
    pub since: Instant,
}

/// What the loop changes as it works, behind one lock so that one task at
/// a time writes the root.
pub(crate) struct Work {
    pub pending: HashMap<PathKey, Pending>,
    pub selection: Selection,
    /// Blobs being fetched, with the keys waiting for each.
    pub fetching: HashMap<ContentHash, BTreeSet<PathKey>>,
    /// Blobs added since the protected set was last computed.
    pub tags: Vec<TempTag>,
    pub join: JoinState,
    pub protect_due: bool,
}

/// Work for the loop from outside it.
pub(crate) enum Wake {
    Keys(Vec<PathKey>),
}

pub(crate) struct Inner {
    pub data: DataDir,
    pub config: GroupConfig,
    pub group: GroupId,
    pub machine: SecretKey,
    pub clock: Clock,
    pub state: State,
    pub ledger: Arc<SharedLedger>,
    pub blobs: Blobs,
    pub node: Node,
    pub options: Options,
    pub work: tokio::sync::Mutex<Work>,
    pub wake: mpsc::UnboundedSender<Wake>,
    pub errors: Mutex<VecDeque<String>>,
    pub started: Instant,
    _mdns: Option<MdnsAddressLookup>,
}

const ERRORS_KEPT: usize = 100;
const PROTECT_EVERY: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_millis(200);

/// The current time in NTP64.
pub(crate) fn now() -> u64 {
    ntp_time(SystemTime::now())
}

impl Inner {
    pub(crate) fn me(&self) -> MachineId {
        self.machine.public()
    }

    /// Keeps an error for the status view.
    pub(crate) fn report(&self, error: impl std::fmt::Display) {
        let mut errors = self.errors.lock().expect("no panic holds the errors");
        if errors.len() == ERRORS_KEPT {
            errors.pop_front();
        }
        errors.push_back(error.to_string());
    }

    /// Signs, stores, folds and sends a patch of this machine.
    pub(crate) fn publish_at(
        &self,
        stamp: Stamp,
        changes: Vec<Change>,
        applies: Option<GroupPath>,
    ) -> Result<()> {
        let patch = Patch {
            stamp,
            changes,
            applies,
        };
        let signed = SignedPatch::sign(&self.group, patch, self.config.cert.clone(), &self.machine);
        self.state.add_patch(&signed)?;
        self.ledger.lock().insert(signed.clone())?;
        self.node.publish(vec![signed]);
        Ok(())
    }

    /// Stores `bytes` as a blob and returns them as file content.
    pub(crate) async fn add_content(&self, work: &mut Work, bytes: Vec<u8>) -> Result<Content> {
        let size = bytes.len() as u64;
        let tag = self.blobs.add_bytes(bytes).await?;
        let content = Content {
            hash: ContentHash(*tag.hash().as_bytes()),
            size,
            executable: false,
        };
        work.tags.push(tag);
        work.protect_due = true;
        Ok(content)
    }

    /// Where the member stands, from the ledger.
    pub(crate) fn join_state(&self) -> JoinState {
        let ledger = self.ledger.lock();
        let name = &self.config.member;
        if let Some(member) = ledger.members().get(name) {
            let by = member.rebound.as_ref().map(|rebinding| &rebinding.by);
            return match (member.key, by) {
                (Some(key), _) if key == self.config.cert.member => JoinState::Joined,
                (Some(_), None) => {
                    JoinState::Taken(format!("the name {name} is taken by another password"))
                }
                (Some(_), Some(by)) => JoinState::Rebound(format!(
                    "{by} gave {name} a new password: log this machine in with it through `pigeon member claim`"
                )),
                (None, Some(by)) if by == name => {
                    JoinState::Excluded(format!("{name} left the group"))
                }
                (None, by) => JoinState::Excluded(format!(
                    "{} excluded {name} from the group",
                    by.map_or("a member", MemberName::as_str)
                )),
            };
        }
        let own_file = member_path(name).key();
        for signed in ledger.patches() {
            let claims = signed.cert.name == *name
                && signed
                    .patch
                    .changes
                    .iter()
                    .any(|change| change.path.key() == own_file);
            if claims && let Some(Err(rejection)) = ledger.outcome(&signed.stamp()) {
                return JoinState::Taken(rejection.to_string());
            }
        }
        JoinState::Pending
    }

    /// Publishes the member file unless the member already belongs.
    async fn join(&self, work: &mut Work) -> Result<()> {
        work.join = self.join_state();
        if work.join != JoinState::Pending {
            return Ok(());
        }
        let statement = MemberStatement {
            name: self.config.member.clone(),
            key: self.config.cert.member,
        };
        let content = self
            .add_content(work, serde_json::to_vec_pretty(&statement)?)
            .await?;
        let path = member_path(&self.config.member);
        let change = Change {
            path: path.clone(),
            content: Some(content),
            replaces: None,
        };
        self.publish_at(self.clock.stamp(), vec![change], None)?;
        work.join = self.join_state();
        let _ = self.wake.send(Wake::Keys(vec![path.key()]));
        Ok(())
    }

    /// Takes patches from a peer: stores and folds the new ones, passes
    /// them on, and brings the paths they touch into agreement.
    async fn receive(self: &Arc<Self>, work: &mut Work, received: Received) {
        let mut fresh = Vec::new();
        let mut keys = BTreeSet::new();
        for signed in received.patches {
            let stamp = signed.stamp();
            if self.ledger.lock().patch(&stamp).is_some() {
                continue;
            }
            if let Err(error) = self.clock.observe(&stamp) {
                self.report(format!("refused a patch from {}: {error}", received.from));
                continue;
            }
            let folded = match self.ledger.lock().insert(signed.clone()) {
                Ok(folded) => folded,
                Err(error) => {
                    self.report(format!("refused a patch from {}: {error}", received.from));
                    continue;
                }
            };
            if let Err(error) = self.state.add_patch(&signed) {
                self.report(error);
            }
            let ledger = self.ledger.lock();
            for stamp in folded {
                if let Some(patch) = ledger.patch(&stamp) {
                    keys.extend(patch.patch.changes.iter().map(|change| change.path.key()));
                }
            }
            drop(ledger);
            fresh.push(signed);
        }
        if fresh.is_empty() {
            return;
        }
        self.node.publish(fresh);
        self.want_peers();
        work.join = self.join_state();
        self.renew_secret(work);
        let keys: Vec<PathKey> = keys.into_iter().collect();
        self.refresh_keys(work, &keys).await;
        if keys.iter().any(|key| key.as_str().starts_with(STATEMENTS)) {
            self.apply_requests(work).await;
        }
        work.protect_due = true;
    }

    /// Draws a new group secret if a member was excluded since the one held
    /// was made, unless this machine's member no longer belongs.
    pub(crate) fn renew_secret(&self, work: &Work) {
        if work.join != JoinState::Joined {
            return;
        }
        let Some(exclusion) = self.ledger.lock().last_exclusion() else {
            return;
        };
        if self.node.secret().borrow().predates(exclusion) {
            let _ = self.node.offer(RenewedSecret {
                secret: random_secret(),
                renewal: Some(Renewal {
                    after: exclusion,
                    by: self.me(),
                }),
            });
        }
    }

    /// Keeps the group secret the node holds in the configuration.
    fn save_secret(&self, secret: RenewedSecret) -> Result<()> {
        let mut config = self.data.load_config()?;
        if secret.supersedes(&config.secret()) {
            config.renew(secret);
            self.data.save_config(&config)?;
        }
        Ok(())
    }

    /// Keeps sessions open with every machine the ledger or the key names.
    fn want_peers(&self) {
        let me = self.me();
        let vector = self.ledger.lock().vector();
        let machines: HashSet<MachineId> = vector
            .into_keys()
            .chain(self.config.key.bootstrap.iter().copied())
            .filter(|machine| *machine != me)
            .collect();
        self.node.want(machines);
    }

    /// One pass of the timer: join when due, publish settled edits,
    /// rescan and protect when due.
    async fn tick(
        self: &Arc<Self>,
        work: &mut Work,
        last_rescan: &mut Instant,
        last_protect: &mut Instant,
    ) {
        if work.join == JoinState::Pending
            && self.started.elapsed() >= self.options.join_delay
            && let Err(error) = self.join(work).await
        {
            self.report(format!("joining: {error}"));
        }
        self.publish_settled(work, &[]).await;
        if last_rescan.elapsed() >= self.options.rescan {
            *last_rescan = Instant::now();
            self.refresh(work, &Rescan::All).await;
            self.apply_requests(work).await;
        }
        if work.protect_due || last_protect.elapsed() >= PROTECT_EVERY {
            *last_protect = Instant::now();
            if let Err(error) = self.protect(work).await {
                self.report(format!("protecting blobs: {error}"));
            }
        }
    }
}

/// One group running on this machine.
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Engine {
    /// Opens the group kept in `data` and starts syncing its root.
    ///
    /// # Errors
    ///
    /// Fails if the state cannot be opened, the root created or watched, or
    /// the endpoint bound.
    pub async fn start(data: &DataDir, options: Options) -> Result<Self> {
        let config = data.load_config()?;
        let machine = data.machine_key()?;
        std::fs::create_dir_all(&config.root)
            .with_context(|| format!("creating {}", config.root.display()))?;
        let state = State::open(&data.state_path())?;
        let group = config.key.group;
        let ledger = state.ledger(group)?;
        let clock = Clock::new(machine.public(), options.max_drift);
        if let Some(time) = ledger.vector().get(&machine.public()) {
            let _ = clock.observe(&Stamp {
                time: *time,
                machine: machine.public(),
            });
        }
        let mut selection = state.selection()?;
        if selection.rules().next().is_none() {
            selection.set(Rule {
                pattern: format!("{}/", config.member.folder()),
                cutoff: Cutoff::PlusInfinity,
            })?;
            state.set_selection(&selection)?;
        }
        let ledger = Arc::new(SharedLedger(Mutex::new(ledger)));
        let blobs = Blobs::open(&data.blobs_path(), options.gc).await?;
        let (endpoint, mdns) = match &options.network {
            Network::Internet => {
                let (endpoint, mdns) = bind_internet(machine.clone(), &group).await?;
                (endpoint, Some(mdns))
            }
            Network::Local(lookup) => (bind_local(machine.clone(), lookup).await?, None),
        };
        let (node, received) = Node::spawn(
            endpoint,
            group,
            config.cert.clone(),
            config.secret(),
            ledger.clone(),
            blobs.store(),
        );
        if let Some(mdns) = &mdns {
            node.follow(mdns);
        }
        let (wake, wakes) = mpsc::unbounded_channel();
        let (rescans, rescan_events) = mpsc::unbounded_channel();
        let watcher = watch(&config.root, rescans)?;
        let inner = Arc::new(Inner {
            data: data.clone(),
            config,
            group,
            machine,
            clock,
            state,
            ledger,
            blobs,
            node,
            options,
            work: tokio::sync::Mutex::new(Work {
                pending: HashMap::new(),
                selection,
                fetching: HashMap::new(),
                tags: Vec::new(),
                join: JoinState::Pending,
                protect_due: true,
            }),
            wake,
            errors: Mutex::new(VecDeque::new()),
            started: Instant::now(),
            _mdns: mdns,
        });
        {
            let mut work = inner.work.lock().await;
            work.join = inner.join_state();
            inner.renew_secret(&work);
        }
        inner.want_peers();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(run(
            inner.clone(),
            received,
            rescan_events,
            wakes,
            stopped,
            watcher,
        ));
        Ok(Self {
            inner,
            stop: Some(stop),
            task: Some(task),
        })
    }

    /// This machine's id.
    #[must_use]
    pub fn machine(&self) -> MachineId {
        self.inner.me()
    }

    /// Stops the loop and closes every session; closing the blob protocol
    /// flushes the blob store.
    ///
    /// # Errors
    ///
    /// Fails if the network does not shut down cleanly.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await?;
        }
        self.inner.node.shutdown().await?;
        Ok(())
    }
}

/// Merges the rescans that arrive within the debounce window.
fn merge(rescans: &mut Vec<Rescan>, rescan: Rescan) {
    match rescan {
        Rescan::All => {
            rescans.clear();
            rescans.push(Rescan::All);
        }
        Rescan::Under(path) => {
            let covered = rescans.iter().any(|known| match known {
                Rescan::All => true,
                Rescan::Under(known) => *known == path || path.is_inside(known.as_str()),
            });
            if !covered {
                rescans.retain(|known| {
                    !matches!(known, Rescan::Under(known) if known.is_inside(path.as_str()))
                });
                rescans.push(Rescan::Under(path));
            }
        }
    }
}

async fn run(
    inner: Arc<Inner>,
    mut received: mpsc::Receiver<Received>,
    mut rescan_events: mpsc::UnboundedReceiver<Rescan>,
    mut wakes: mpsc::UnboundedReceiver<Wake>,
    mut stopped: oneshot::Receiver<()>,
    _watcher: notify::RecommendedWatcher,
) {
    let mut secrets: watch::Receiver<RenewedSecret> = inner.node.secret();
    secrets.mark_changed();
    let mut ticks = tokio::time::interval(inner.options.tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_rescan = Instant::now();
    let mut last_protect = Instant::now();
    {
        let mut work = inner.work.lock().await;
        inner.refresh(&mut work, &Rescan::All).await;
        inner.apply_requests(&mut work).await;
    }
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            Ok(()) = secrets.changed() => {
                let secret = secrets.borrow_and_update().clone();
                if let Err(error) = inner.save_secret(secret) {
                    inner.report(format!("keeping the group secret: {error}"));
                }
            }
            Some(patches) = received.recv() => {
                let mut work = inner.work.lock().await;
                inner.receive(&mut work, patches).await;
            }
            Some(rescan) = rescan_events.recv() => {
                let mut rescans = vec![rescan];
                tokio::time::sleep(DEBOUNCE).await;
                while let Ok(rescan) = rescan_events.try_recv() {
                    merge(&mut rescans, rescan);
                }
                let mut work = inner.work.lock().await;
                for rescan in &rescans {
                    inner.refresh(&mut work, rescan).await;
                }
            }
            Some(wake) = wakes.recv() => {
                let mut work = inner.work.lock().await;
                let Wake::Keys(keys) = wake;
                inner.refresh_keys(&mut work, &keys).await;
                if keys.iter().any(|key| key.as_str().starts_with(STATEMENTS)) {
                    inner.apply_requests(&mut work).await;
                }
            }
            _ = ticks.tick() => {
                let mut work = inner.work.lock().await;
                inner.tick(&mut work, &mut last_rescan, &mut last_protect).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn under(path: &str) -> Rescan {
        Rescan::Under(GroupPath::parse(path).unwrap())
    }

    #[test]
    fn merged_rescans_cover_each_change_once() {
        let mut rescans = Vec::new();
        merge(&mut rescans, under("a/b"));
        merge(&mut rescans, under("a/b/c"));
        merge(&mut rescans, under("d"));
        assert_eq!(rescans.len(), 2);
        merge(&mut rescans, under("a"));
        assert!(
            matches!(&rescans[..], [Rescan::Under(d), Rescan::Under(a)] if d.as_str() == "d" && a.as_str() == "a")
        );
        merge(&mut rescans, Rescan::All);
        assert!(matches!(&rescans[..], [Rescan::All]));
    }
}
