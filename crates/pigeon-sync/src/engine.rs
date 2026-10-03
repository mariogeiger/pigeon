//! The engine of one group on one machine: it opens the group's state,
//! binds its endpoint, and runs the one loop that receives patches, follows
//! the root's changes, publishes settled edits or suggests them, follows
//! the suggestions, joins the member to the group, and keeps the blobs it
//! needs from garbage collection. After each turn it announces this
//! machine's drafts and signals whether what the engine shows may have
//! changed.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use iroh::Endpoint;
use iroh::address_lookup::MemoryLookup;
use iroh_base::SecretKey;
use iroh_blobs::api::TempTag;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use pigeon_core::clock::{Clock, MachineId, Stamp, ntp_time};
use pigeon_core::identity::{GroupId, MachineCert};
use pigeon_core::ledger::{Digests, Ledger};
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Change, Content, ContentHash, Patch, SignedPatch};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::places::Places;
use pigeon_core::statement::{MemberStatement, is_relay_path, is_statement, member_path};
use pigeon_net::bind::{bind_internet, bind_local};
use pigeon_net::hello::Announcement;
use pigeon_net::wire::Patches;
use pigeon_net::{Log, Node, Received, Timings};
use pigeon_store::blobs::Blobs;
use pigeon_store::config::ConfigFile;
use pigeon_store::disk::Stat;
use pigeon_store::group_dirs::GroupDirs;
use pigeon_store::group_key::GroupKey;
use pigeon_store::probe::Unportable;
use pigeon_store::state::State;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::receive::{take, wanted};
use crate::root::open_root;
use crate::suggestions::Live;
use crate::watch::{Rescan, Watched};

/// How the endpoint finds other machines.
#[derive(Clone, Debug)]
pub enum Network {
    /// Relays, address lookup, and discovery on the local network.
    Internet,
    /// Only the machines added to this lookup, on this host.
    Local(MemoryLookup),
}

/// Binds the endpoint of `machine` in `group` on `network`, with the
/// discovery on the local network that the internet brings.
pub(crate) async fn bind(
    machine: &SecretKey,
    group: &GroupId,
    network: &Network,
) -> Result<(Endpoint, Option<MdnsAddressLookup>)> {
    Ok(match network {
        Network::Internet => {
            let (endpoint, mdns) = bind_internet(machine.clone(), group).await?;
            (endpoint, Some(mdns))
        }
        Network::Local(lookup) => (bind_local(machine.clone(), lookup).await?, None),
    })
}

/// The engine's network, what it tells machines that ask which pigeon it
/// runs, and its timings.
#[derive(Clone, Debug)]
pub struct Options {
    pub network: Network,
    pub announcement: Announcement,
    /// How long an edit in a personal folder must stay unchanged before it
    /// is published.
    pub settle_personal: Duration,
    /// The same for a draft, a new file at a path no member owns.
    pub settle_draft: Duration,
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
    /// How often sessions compare what both sides hold, and how long a
    /// fetch waits for its blob to grow.
    pub node: Timings,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            network: Network::Internet,
            announcement: Announcement::speaking_ours(env!("CARGO_PKG_VERSION"), "unknown"),
            settle_personal: Duration::from_secs(3),
            settle_draft: Duration::from_secs(300),
            rescan: Duration::from_secs(600),
            tick: Duration::from_secs(1),
            join_delay: Duration::from_secs(5),
            gc: Duration::from_secs(3600),
            max_drift: Duration::from_secs(300),
            node: Timings::default(),
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
    /// Another key holds the name, or a folder claims it.
    Taken(String),
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
    pub(crate) fn new(ledger: Ledger) -> Self {
        Self(Mutex::new(ledger))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Ledger> {
        self.0.lock().expect("no panic holds the ledger")
    }
}

impl Log for SharedLedger {
    fn digests(&self) -> Digests {
        self.lock().digests()
    }

    fn missing_from(&self, theirs: &Digests) -> Patches {
        self.lock()
            .missing_from(theirs)
            .into_iter()
            .cloned()
            .collect()
    }

    fn recognizes(&self, cert: &MachineCert) -> bool {
        self.lock().recognizes(cert)
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

impl Pending {
    /// The bytes of the file it leaves, none for a deletion.
    pub fn size(&self) -> u64 {
        self.stat.as_ref().map_or(0, |stat| stat.size)
    }
}

/// What the loop changes as it works, behind one lock so that one task at
/// a time writes the root.
pub(crate) struct Work {
    pub pending: HashMap<PathKey, Pending>,
    /// The selection, retention and places, as `config.toml` holds them.
    pub config: ConfigFile,
    /// Blobs being fetched, with the keys waiting for each.
    pub fetching: HashMap<ContentHash, BTreeSet<PathKey>>,
    /// How many fetches of each blob failed in a row, until one succeeds;
    /// at most one entry per content the ledger names.
    pub fetch_failures: HashMap<ContentHash, u32>,
    /// The tasks fetching blobs, which shutting down ends.
    pub fetches: JoinSet<()>,
    /// Blobs added since the protected set was last computed.
    pub tags: Vec<TempTag>,
    pub join: JoinState,
    pub protect_due: bool,
    /// The folders the disk holds at other destinations.
    pub placed: Places,
    /// The folders out of place, with why; nothing under them syncs.
    pub out_of_place: Vec<(GroupPath, String)>,
    /// Why the whole root is out of place, if it is; nothing syncs then.
    pub root_problem: Option<String>,
    pub watcher: Option<Arc<notify::RecommendedWatcher>>,
    pub watched: Vec<Watched>,
    /// The drafts last announced: each path, size, and when it last changed.
    pub announced: Option<Vec<(PathKey, u64, Instant)>>,
    /// The live suggestions read so far, by their statements' keys.
    pub suggestions: BTreeMap<PathKey, Live>,
    /// The names the scans kept out of the group, by path.
    pub unportable: BTreeMap<String, Unportable>,
}

impl Work {
    /// The work of an engine that starts with `config`, the disk holding
    /// the folders `placed` at other destinations.
    fn starting(config: ConfigFile, placed: Places) -> Self {
        Self {
            pending: HashMap::new(),
            config,
            fetching: HashMap::new(),
            fetch_failures: HashMap::new(),
            fetches: JoinSet::new(),
            tags: Vec::new(),
            join: JoinState::Pending,
            protect_due: true,
            placed,
            out_of_place: Vec::new(),
            root_problem: None,
            watcher: None,
            watched: Vec::new(),
            announced: None,
            suggestions: BTreeMap::new(),
            unportable: BTreeMap::new(),
        }
    }
}

/// What the status view reads of [`Work`], copied each time the engine
/// signals, so that reading it waits for no work in progress.
#[derive(Clone)]
pub(crate) struct Glance {
    pub join: JoinState,
    pub pending: usize,
    pub fetching: usize,
    pub paused: Option<String>,
}

impl Glance {
    /// Before the first pass: joining, with nothing pending.
    fn starting() -> Self {
        Self {
            join: JoinState::Pending,
            pending: 0,
            fetching: 0,
            paused: None,
        }
    }
}

/// How many passes of each recurring kind of work the engine finished,
/// counts that only grow, so that whoever waits for the engine to have
/// looked at the disk or published what settled waits for them to grow.
#[derive(Default)]
pub(crate) struct Passes {
    /// The passes of the timer, each publishing or suggesting what settled.
    pub ticks: AtomicU64,
    /// The scans of the disk, of the whole root or of some paths, each
    /// compared with the ledger.
    pub scans: AtomicU64,
}

impl Passes {
    pub(crate) fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) struct Inner {
    /// The member this machine speaks for.
    pub member: MemberName,
    pub root: std::path::PathBuf,
    /// The member's certificate of this machine.
    pub cert: MachineCert,
    /// The group key the machine started with.
    pub key: GroupKey,
    pub group: GroupId,
    pub machine: SecretKey,
    pub clock: Clock,
    /// The first stamp of this run: every later patch signed with this
    /// machine's key was made by this run, and is in the ledger.
    pub first_stamp: Stamp,
    pub state: State,
    pub ledger: Arc<SharedLedger>,
    pub blobs: Blobs,
    pub node: Node,
    pub options: Options,
    pub work: tokio::sync::Mutex<Work>,
    /// Keys to bring into agreement, sent from outside the loop.
    pub wake: mpsc::UnboundedSender<Vec<PathKey>>,
    pub rescans: mpsc::UnboundedSender<Rescan>,
    pub errors: Mutex<VecDeque<String>>,
    pub passes: Passes,
    pub started: Instant,
    /// A fingerprint of what the engine shows, sent anew when it changes.
    pub changes: watch::Sender<u64>,
    pub glance: Mutex<Glance>,
    _mdns: Option<MdnsAddressLookup>,
}

const ERRORS_KEPT: usize = 100;
/// How far the newest patch held may lie ahead of this machine's clock
/// before the engine reports it.
const LAG_REPORTED: Duration = Duration::from_secs(1);
const PROTECT_EVERY: Duration = Duration::from_secs(60);
/// How long the rescans the watcher asks for gather, from the first, before
/// the loop makes them in one look, doing its other work meanwhile.
const DEBOUNCE: Duration = Duration::from_millis(200);
/// The most subtrees one look rescans after a burst of changes; a larger
/// burst rescans the whole tree.
const RESCANS_KEPT: usize = 256;

/// The current time in NTP64.
pub(crate) fn now() -> u64 {
    ntp_time(SystemTime::now())
}

impl Inner {
    pub(crate) fn me(&self) -> MachineId {
        self.machine.public()
    }

    /// Keeps an error for the status view, as the latest one, once.
    pub(crate) fn report(&self, error: impl std::fmt::Display) {
        let error = error.to_string();
        let mut errors = self.errors.lock().expect("no panic holds the errors");
        errors.retain(|kept| *kept != error);
        if errors.len() == ERRORS_KEPT {
            errors.pop_front();
        }
        errors.push_back(error);
    }

    /// A hash of everything the views read: the state's writes, the
    /// configuration, the pending edits, the fetches, the errors, the
    /// member's standing, the folders out of place, the drafts other
    /// machines announced, the peers, why others failed, and the relay.
    fn fingerprint(&self, work: &Work) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.state.revision().hash(&mut hasher);
        work.config.text().hash(&mut hasher);
        let mut pending: Vec<(&PathKey, &Instant)> = work
            .pending
            .iter()
            .map(|(key, pending)| (key, &pending.since))
            .collect();
        pending.sort_unstable();
        pending.hash(&mut hasher);
        let mut fetching: Vec<&BTreeSet<PathKey>> = work.fetching.values().collect();
        fetching.sort_unstable();
        fetching.hash(&mut hasher);
        self.errors
            .lock()
            .expect("no panic holds the errors")
            .hash(&mut hasher);
        format!(
            "{:?}{:?}{:?}",
            work.join, work.out_of_place, work.root_problem
        )
        .hash(&mut hasher);
        self.node.announced().hash(&mut hasher);
        let mut peers = self.node.peers();
        peers.sort_unstable();
        peers.hash(&mut hasher);
        self.node.failures().hash(&mut hasher);
        self.node
            .home_relay()
            .map(|url| url.to_string())
            .hash(&mut hasher);
        hasher.finish()
    }

    /// Keeps the join state the ledger tells as the one the status view
    /// reads, until the first signal.
    pub(crate) fn glance_join(&self) {
        self.glance.lock().expect("no panic holds the glance").join = self.join_state();
    }

    /// Sends the fingerprint when it changed, and keeps what the status
    /// view reads of `work`.
    pub(crate) fn signal(&self, work: &Work) {
        *self.glance.lock().expect("no panic holds the glance") = Glance {
            join: work.join.clone(),
            pending: work.pending.len(),
            fetching: work.fetching.len(),
            paused: work.root_problem.clone(),
        };
        let fingerprint = self.fingerprint(work);
        self.changes.send_if_modified(|sent| {
            let changed = *sent != fingerprint;
            *sent = fingerprint;
            changed
        });
    }

    /// Signs, stores, folds and sends a patch of this machine, once the
    /// blobs it names are on the disk for good, so that no stored patch
    /// names content this machine lost.
    pub(crate) async fn publish_at(&self, stamp: Stamp, changes: Vec<Change>) -> Result<()> {
        self.blobs.store().sync_db().await?;
        let patch = Patch { stamp, changes };
        let signed = SignedPatch::sign(&self.group, patch, self.cert.clone(), &self.machine);
        self.state.add_patch(&signed)?;
        self.ledger.lock().insert(signed.clone())?;
        self.node.publish(vec![signed]);
        Ok(())
    }

    /// Stores `bytes` as a blob and returns them as file content.
    pub(crate) async fn add_content(&self, work: &mut Work, bytes: Vec<u8>) -> Result<Content> {
        let size = bytes.len() as u64;
        let tag = self.blobs.add_bytes(bytes).await?;
        Ok(Self::keep_content(work, tag, size))
    }

    /// Stores the file at `from`, read in chunks, and returns it as file
    /// content.
    pub(crate) async fn add_file(&self, work: &mut Work, from: &Path) -> Result<Content> {
        let (tag, size) = self.blobs.import(from).await?;
        Ok(Self::keep_content(work, tag, size))
    }

    /// Keeps the stored content safe from collection until it is protected.
    fn keep_content(work: &mut Work, tag: TempTag, size: u64) -> Content {
        let content = Content {
            hash: ContentHash(*tag.hash().as_bytes()),
            size,
            executable: false,
        };
        work.tags.push(tag);
        work.protect_due = true;
        content
    }

    /// Where the member stands, from the ledger.
    pub(crate) fn join_state(&self) -> JoinState {
        let ledger = self.ledger.lock();
        let name = &self.member;
        if let Some(member) = ledger.members().get(name) {
            return if member.key == self.cert.member {
                JoinState::Joined
            } else {
                JoinState::Taken(format!("the name {name} is taken by another key"))
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

    /// Fails unless the member has joined, as only members publish.
    pub(crate) fn ensure_joined(&self, work: &Work) -> Result<()> {
        if work.join != JoinState::Joined {
            bail!("{} has not joined the group yet", self.member);
        }
        Ok(())
    }

    /// Publishes the member file unless the member already belongs.
    async fn join(&self, work: &mut Work) -> Result<()> {
        work.join = self.join_state();
        if work.join != JoinState::Pending {
            return Ok(());
        }
        let statement = MemberStatement {
            name: self.member.clone(),
            key: self.cert.member,
        };
        self.publish_statement(
            work,
            self.clock.stamp(),
            member_path(&self.member),
            serde_json::to_vec_pretty(&statement)?,
        )
        .await?;
        work.join = self.join_state();
        Ok(())
    }

    /// Takes patches from a peer: stores and folds the new ones, passes
    /// them on, suggests the changes of this machine they made lose, and
    /// brings the paths they touch into agreement.
    async fn receive(self: &Arc<Self>, work: &mut Work, received: Received) {
        let taken = take(
            &self.ledger,
            |new| self.state.add_patches(new),
            &self.clock,
            &self.first_stamp,
            received,
        );
        for refusal in taken.refusals {
            self.report(refusal);
        }
        if taken.fresh.is_empty() {
            return;
        }
        self.node.publish(taken.fresh);
        self.want_peers();
        work.join = self.join_state();
        self.suggest_losses(work).await;
        let keys: Vec<PathKey> = taken.keys.into_iter().collect();
        self.refresh_keys(work, &keys).await;
        if keys.iter().any(is_statement) {
            self.follow_suggestions(work).await;
        }
        work.protect_due = true;
    }

    /// Keeps sessions open with every machine the ledger or the key names.
    fn want_peers(&self) {
        let machines = wanted(&self.ledger.lock(), &self.key, self.me());
        self.node.want(machines);
    }

    /// One pass of the timer: join when due, publish or suggest settled
    /// edits, and protect when due.
    async fn tick(self: &Arc<Self>, work: &mut Work, last_protect: &mut Instant) {
        self.pause_without_root(work);
        if work.join == JoinState::Pending
            && self.started.elapsed() >= self.options.join_delay
            && let Err(error) = self.join(work).await
        {
            self.report(format!("joining: {error}"));
        }
        self.publish_settled(work, &[]).await;
        if work.protect_due || last_protect.elapsed() >= PROTECT_EVERY {
            *last_protect = Instant::now();
            if let Err(error) = self.protect(work).await {
                self.report(format!("protecting blobs: {error}"));
            }
        }
        Passes::count(&self.passes.ticks);
    }
}

/// One group running on this machine.
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Engine {
    /// Opens the group kept in `data` and starts syncing its root as its
    /// configuration says, returning once the engine's first pass over the
    /// disk holds the work, which every view and action of the engine then
    /// comes after.
    ///
    /// # Errors
    ///
    /// Fails if the configuration or the secrets are invalid, the state
    /// cannot be opened, the root created, or the endpoint bound.
    pub async fn start(dirs: &GroupDirs, options: Options) -> Result<Self> {
        let secrets = dirs.secrets()?;
        let Some(key) = secrets.key.clone() else {
            bail!(
                "{} holds no group key: join the group with `pigeon group join`",
                dirs.secrets_path().display()
            );
        };
        let machine = secrets.machine.clone();
        let state = State::open(&dirs.state_path())?;
        let group = key.group;
        let ledger = state.ledger(group)?;
        let clock = Clock::new(machine.public(), options.max_drift);
        if let Some(newest) = ledger.newest() {
            clock.observe(newest);
        }
        let lead = clock.lead();
        let first_stamp = clock.stamp();
        let config = ConfigFile::open(dirs)?;
        open_root(&state, &config.root)?;
        let cert = MachineCert::derive(&group, config.member.clone(), machine.public());
        let ledger = Arc::new(SharedLedger::new(ledger));
        let blobs = Blobs::open(&dirs.blobs_path(), options.gc).await?;
        let (endpoint, mdns) = bind(&machine, &group, &options.network).await?;
        let (node, received) = Node::spawn(
            endpoint,
            options.announcement.clone(),
            group,
            key.secret.clone(),
            ledger.clone(),
            blobs.store(),
            options.node,
        );
        if let Some(mdns) = &mdns {
            node.dial_discovered(mdns);
        }
        let (wake, wakes) = mpsc::unbounded_channel();
        let (rescans, rescan_events) = mpsc::unbounded_channel();
        let placed = state.placed()?;
        let inner = Arc::new(Inner {
            member: config.member.clone(),
            root: config.root.clone(),
            cert,
            key,
            group,
            machine,
            clock,
            first_stamp,
            state,
            ledger,
            blobs,
            node,
            options,
            work: tokio::sync::Mutex::new(Work::starting(config, placed)),
            wake,
            rescans,
            errors: Mutex::new(VecDeque::new()),
            passes: Passes::default(),
            started: Instant::now(),
            changes: watch::Sender::new(0),
            glance: Mutex::new(Glance::starting()),
            _mdns: mdns,
        });
        if lead > LAG_REPORTED {
            inner.report(format!(
                "the newest patch this machine holds is dated {}s after its clock: one of the \
                 clocks is wrong; this machine dates its patches after that one meanwhile",
                lead.as_secs()
            ));
        }
        inner.work.lock().await.join = inner.join_state();
        inner.glance_join();
        inner.want_peers();
        let (stop, stopped) = oneshot::channel();
        let (begun, first_pass) = oneshot::channel();
        let task = tokio::spawn(run(
            inner.clone(),
            received,
            rescan_events,
            wakes,
            (begun, stopped),
        ));
        first_pass.await?;
        Ok(Self {
            inner,
            stop: Some(stop),
            task: Some(task),
        })
    }

    /// A fingerprint of what the engine shows, which changes whenever that
    /// may have, at the latest one tick after.
    #[must_use]
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    /// This machine's id.
    #[must_use]
    pub fn machine(&self) -> MachineId {
        self.inner.me()
    }

    /// Stops the loop, ends the fetches, closes every session and blob
    /// connection, then stops the blob store whole, flushing it, so that
    /// nothing holds the group's state once it returns and no request
    /// reaches a stopped store.
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
        let mut fetches = std::mem::take(&mut self.inner.work.lock().await.fetches);
        fetches.shutdown().await;
        let closed = self.inner.node.shutdown().await;
        let blobs = self.inner.blobs.clone();
        drop(self);
        blobs.stop_whole().await;
        closed
    }
}

/// Merges the rescans that arrive within the debounce window, at most
/// `RESCANS_KEPT` subtrees before the whole tree.
fn merge(rescans: &mut Vec<Rescan>, rescan: Rescan) {
    let Rescan::Under(path) = rescan else {
        *rescans = vec![Rescan::All];
        return;
    };
    let covered = rescans.iter().any(|known| match known {
        Rescan::All => true,
        Rescan::Under(known) => path.is_within(known),
    });
    if covered {
        return;
    }
    rescans.retain(|known| !matches!(known, Rescan::Under(known) if known.is_within(&path)));
    if rescans.len() < RESCANS_KEPT {
        rescans.push(Rescan::Under(path));
    } else {
        *rescans = vec![Rescan::All];
    }
}

async fn run(
    inner: Arc<Inner>,
    mut received: mpsc::Receiver<Received>,
    mut rescan_events: mpsc::UnboundedReceiver<Rescan>,
    mut wakes: mpsc::UnboundedReceiver<Vec<PathKey>>,
    (begun, mut stopped): (oneshot::Sender<()>, oneshot::Receiver<()>),
) {
    let mut ticks = tokio::time::interval(inner.options.tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_rescan = Instant::now();
    let mut last_protect = Instant::now();
    let mut gathered = Vec::new();
    let mut gathered_until = None;
    {
        let mut work = inner.work.lock().await;
        let _ = begun.send(());
        inner.follow_relay().await;
        inner.lay_out(&mut work).await;
        if let Err(error) = inner
            .applied_selection(&work)
            .and_then(|before| inner.free_unselected(&work, &before))
        {
            inner.report(format!(
                "freeing what the selection no longer holds: {error}"
            ));
        }
        inner.follow_suggestions(&mut work).await;
        inner.refresh(&mut work, &[Rescan::All]).await;
    }
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            Some(patches) = received.recv() => {
                let relayed = patches.patches.iter().any(|signed| {
                    signed.patch.changes.iter().any(|change| is_relay_path(&change.path))
                });
                let mut work = inner.work.lock().await;
                inner.receive(&mut work, patches).await;
                drop(work);
                if relayed {
                    inner.follow_relay().await;
                }
            }
            Some(rescan) = rescan_events.recv() => {
                merge(&mut gathered, rescan);
                gathered_until.get_or_insert_with(|| tokio::time::Instant::now() + DEBOUNCE);
                continue;
            }
            () = tokio::time::sleep_until(gathered_until.unwrap_or_else(tokio::time::Instant::now)),
                if gathered_until.is_some() => {
                gathered_until = None;
                inner.rescan(&std::mem::take(&mut gathered)).await;
            }
            Some(keys) = wakes.recv() => {
                let mut work = inner.work.lock().await;
                inner.refresh_keys(&mut work, &keys).await;
                if keys.iter().any(is_statement) {
                    inner.follow_suggestions(&mut work).await;
                    inner.follow_relay().await;
                }
            }
            _ = ticks.tick() => {
                let mut work = inner.work.lock().await;
                inner.tick(&mut work, &mut last_protect).await;
                drop(work);
                if last_rescan.elapsed() >= inner.options.rescan {
                    last_rescan = Instant::now();
                    inner.rescan(&[Rescan::All]).await;
                    inner.follow_suggestions(&mut *inner.work.lock().await).await;
                }
            }
        }
        let mut work = inner.work.lock().await;
        inner.announce_drafts(&mut work);
        inner.signal(&work);
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

    #[test]
    fn a_burst_past_the_bound_rescans_the_whole_tree_once() {
        let mut rescans = Vec::new();
        for index in 0..RESCANS_KEPT {
            merge(&mut rescans, under(&format!("f{index}")));
        }
        assert_eq!(rescans.len(), RESCANS_KEPT);
        merge(&mut rescans, under("f0"));
        assert_eq!(rescans.len(), RESCANS_KEPT);
        merge(&mut rescans, under("one more"));
        assert!(matches!(&rescans[..], [Rescan::All]));
        merge(&mut rescans, under("f1"));
        assert!(matches!(&rescans[..], [Rescan::All]));
    }
}
