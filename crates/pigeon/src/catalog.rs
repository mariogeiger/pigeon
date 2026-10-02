//! Every action pigeon offers, defined once with its name, arguments, what
//! its results show and the handler that carries it out: the command line,
//! the JSON API and the web UI forms are all generated from this table.

use crate::perform::{self, Handler};

/// What an argument holds, which says how each interface asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// A path in the group, such as `docs/plan.txt`.
    Path,
    /// A gitignore pattern, such as `/docs/` or `*.pdf`.
    Pattern,
    /// A folder on this machine.
    Folder,
    /// A file's content, of any size: a local file on the command line, an
    /// upload in the web UI, the body of the request in the API, which
    /// takes the other arguments in its query. An action has at most one.
    Bytes,
    /// An RFC 3339 time, such as `2026-10-01T12:00:00Z`, or `now`, the
    /// time at which the action is carried out.
    Time,
    /// On or off, off by default.
    Flag,
    /// A text such as a configuration, where an empty one is a value too:
    /// a local file on the command line.
    Document,
}

/// One argument of an action.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub about: &'static str,
    pub kind: Kind,
    pub required: bool,
    /// The action, by noun and verb, whose results list the values this
    /// argument takes, in their first column.
    pub listed_by: Option<(&'static str, &'static str)>,
}

impl Param {
    const fn new(name: &'static str, about: &'static str, kind: Kind, required: bool) -> Self {
        let listed_by = match kind {
            Kind::Path | Kind::Pattern => Some(("file", "list")),
            _ => None,
        };
        Self {
            name,
            about,
            kind,
            required,
            listed_by,
        }
    }

    const fn listed_by(self, noun: &'static str, verb: &'static str) -> Self {
        Self {
            listed_by: Some((noun, verb)),
            ..self
        }
    }
}

/// Whether an action concerns the machine or one of its groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Machine,
    /// The action takes a `group` argument, which may be left out when the
    /// machine has a single group.
    Group,
}

/// One action.
#[derive(Clone, Copy, Debug)]
pub struct Action {
    pub noun: &'static str,
    pub verb: &'static str,
    pub about: &'static str,
    pub scope: Scope,
    pub params: &'static [Param],
    /// Whether the action changes anything, rather than only showing.
    pub changes: bool,
    /// The fields a list result shows, as dotted paths into each item.
    pub columns: &'static [&'static str],
    pub handler: Handler,
}

impl Action {
    /// The command that runs this action, for messages.
    #[must_use]
    pub fn command(&self) -> String {
        format!("pigeon {} {}", self.noun, self.verb)
    }

    #[must_use]
    pub fn param(&self, name: &str) -> Option<&'static Param> {
        self.params.iter().find(|param| param.name == name)
    }
}

/// The argument naming a group.
pub const GROUP: Param = optional(
    "group",
    "The group, which may be left out on a machine with a single group",
    Kind::Text,
)
.listed_by("group", "list");

const fn required(name: &'static str, about: &'static str, kind: Kind) -> Param {
    Param::new(name, about, kind, true)
}

const fn optional(name: &'static str, about: &'static str, kind: Kind) -> Param {
    Param::new(name, about, kind, false)
}

const MEMBER: Param = required(
    "member",
    "Your name in the group: 1 to 32 characters among a-z and 0-9",
    Kind::Text,
);
const KEY: Param = required("key", "The group key a member shared", Kind::Text);
const ROOT: Param = optional(
    "root",
    "The group's folder on this machine, by default /<group>, or C:\\<group> on Windows, the same path on every machine",
    Kind::Folder,
);
const PATTERN: Param = required(
    "pattern",
    "Which files, as a .pigeonignore pattern such as /docs/ or *.pdf",
    Kind::Pattern,
);
const VERSION_TIME: Param = required(
    "time",
    "The time, such as 2026-10-01T12:00:00Z, or now; `pigeon selection times` lists those of the versions",
    Kind::Time,
)
.listed_by("selection", "times");
pub const YES: Param = optional(
    "yes",
    "Go ahead without asking: where the action frees space on this machine, publishes for the whole group, or cannot be undone",
    Kind::Flag,
);
const FOLDER: Param = required(
    "folder",
    "The folder of the group, such as videos",
    Kind::Path,
);
const SUGGESTIONS: Param = required(
    "suggestions",
    "The suggestions, by the ids `pigeon suggestion list` shows, separated by spaces: each is decided as it was then",
    Kind::Text,
)
.listed_by("suggestion", "list");

const FILE_COLUMNS: &[&str] = &[
    "path",
    "owner",
    "author",
    "content.size",
    "time",
    "held",
    "outdated",
];

const fn action(
    noun: &'static str,
    verb: &'static str,
    about: &'static str,
    params: &'static [Param],
    handler: Handler,
) -> Action {
    Action {
        noun,
        verb,
        about,
        scope: Scope::Group,
        params,
        changes: true,
        columns: &[],
        handler,
    }
}

const fn view(
    noun: &'static str,
    verb: &'static str,
    about: &'static str,
    params: &'static [Param],
    columns: &'static [&'static str],
    handler: Handler,
) -> Action {
    Action {
        noun,
        verb,
        about,
        scope: Scope::Group,
        params,
        changes: false,
        columns,
        handler,
    }
}

const fn on_machine(action: Action) -> Action {
    Action {
        scope: Scope::Machine,
        ..action
    }
}

/// What each noun's actions act on.
pub const NOUNS: &[(&str, &str)] = &[
    ("group", "Create, join and inspect groups"),
    ("member", "The group's members"),
    (
        "file",
        "The group's files: list, write, delete, rename and publish them",
    ),
    ("selection", "Which files this machine holds"),
    (
        "config",
        "The group's config.toml on this machine: its member, root, selection, retention and places",
    ),
    (
        "suggestion",
        "The changes the rules leave to the group: anyone validates or discards each, and the first decision counts",
    ),
    (
        "daemon",
        "Run pigeon, which syncs every group, answers the API and serves the web UI, or stop, restart or reload it",
    ),
];

/// Every action, grouped by noun.
pub const ACTIONS: &[Action] = &[
    on_machine(view(
        "group",
        "list",
        "List the groups on this machine",
        &[],
        &["name", "member", "join.state", "peers", "root"],
        Handler::Daemon(perform::list_groups),
    )),
    on_machine(action(
        "group",
        "create",
        "Create a new group, of which you are the first member",
        &[
            required(
                "name",
                "The group's name: 1 to 32 characters among a-z and 0-9",
                Kind::Text,
            ),
            MEMBER,
            ROOT,
        ],
        Handler::Daemon(perform::create_group),
    )),
    on_machine(view(
        "group",
        "names",
        "Hear a group with its key, without joining it, and list the names to join under: a member's, to add a machine of theirs, or any name not taken",
        &[KEY],
        &[],
        Handler::Daemon(perform::hear_names),
    )),
    on_machine(action(
        "group",
        "join",
        "Join a group with the key a member shared",
        &[KEY, MEMBER, ROOT],
        Handler::Daemon(perform::join_group),
    )),
    view(
        "group",
        "status",
        "Show how the group stands on this machine",
        &[],
        &[],
        Handler::Engine(perform::show_status),
    ),
    view(
        "group",
        "key",
        "Show the group key, which admits a new member's machine",
        &[],
        &[],
        Handler::Engine(perform::show_key),
    ),
    action(
        "group",
        "relay",
        "Name a relay that carries, for every machine of the group, what no direct connection can, alongside iroh's public relays",
        &[optional(
            "url",
            "The URL `pigeon relay` printed; leave it out for iroh's public relays alone",
            Kind::Text,
        )],
        Handler::Engine(perform::set_relay),
    ),
    action(
        "group",
        "leave",
        "Leave the group on this machine: it stops syncing and forgets the group's key, secrets and state, keeping its files; your name stays a member's",
        &[YES],
        Handler::Daemon(perform::leave_group),
    ),
    action(
        "group",
        "serve",
        "Make this machine a server of the group: it follows every file and keeps the history of every file it downloads, so that the files stay available while their owners' machines are off",
        &[],
        Handler::Daemon(perform::serve_group),
    ),
    view(
        "member",
        "list",
        "List the members",
        &[],
        &["name", "machines", "online", "joined"],
        Handler::Engine(perform::list_members),
    ),
    action(
        "member",
        "claim",
        "Claim a name for this machine after losing one",
        &[MEMBER],
        Handler::Daemon(perform::claim_name),
    ),
    view(
        "file",
        "list",
        "List the group's files",
        &[optional(
            "under",
            "Only the files in this folder",
            Kind::Path,
        )],
        FILE_COLUMNS,
        Handler::Engine(perform::list_files),
    ),
    view(
        "file",
        "history",
        "List a file's versions, oldest first, back through the paths it moved from",
        &[required("path", "The file", Kind::Path)],
        &["time", "path", "content.size", "author"],
        Handler::Engine(perform::list_history),
    ),
    view(
        "file",
        "pending",
        "List the edits waiting to be published, with the seconds left: this machine's, and the drafts of new files other machines announce, each with the other drafts of its path",
        &[optional(
            "under",
            "Only the edits in this folder",
            Kind::Path,
        )],
        &["path", "author", "due_in", "draft", "deleted"],
        Handler::Engine(perform::list_pending),
    ),
    action(
        "file",
        "publish",
        "Publish waiting edits now rather than once they settle",
        &[optional(
            "path",
            "The file or folder whose edits to publish; leave it out for every edit",
            Kind::Path,
        )],
        Handler::Engine(perform::publish),
    ),
    action(
        "file",
        "write",
        "Write a file, whoever made it",
        &[
            required("path", "The file to write", Kind::Path),
            required(
                "content",
                "A local file holding the new content, or - for standard input",
                Kind::Bytes,
            ),
        ],
        Handler::Engine(perform::write_file),
    ),
    action(
        "file",
        "delete",
        "Delete a file or a folder; their history keeps them",
        &[
            required("path", "The file or folder to delete", Kind::Path),
            YES,
        ],
        Handler::Engine(perform::delete_file),
    ),
    action(
        "file",
        "rename",
        "Rename or move a file or a folder, which keeps its history",
        &[
            required("from", "The file or folder to rename", Kind::Path),
            required("to", "Its new path", Kind::Path),
        ],
        Handler::Engine(perform::rename_file),
    ),
    view(
        "file",
        "unportable",
        "List the names on this machine's disk that some machine cannot hold, which stay out of the group, each with the portable name proposed",
        &[],
        &["path", "reason", "proposal"],
        Handler::Engine(perform::list_unportable),
    ),
    action(
        "file",
        "make-portable",
        "Rename on this machine's disk a name some machine cannot hold to the portable name proposed, which brings what it holds into the group",
        &[required(
            "path",
            "The name, as `pigeon file unportable` lists it",
            Kind::Text,
        )
        .listed_by("file", "unportable")],
        Handler::Engine(perform::make_portable),
    ),
    action(
        "file",
        "restore",
        "Bring files back as they were at a past time, as new versions that undo nothing of the history",
        &[PATTERN, VERSION_TIME, YES],
        Handler::Engine(perform::restore),
    ),
    action(
        "selection",
        "follow",
        "Keep files in sync on this machine",
        &[PATTERN],
        Handler::Engine(perform::follow),
    ),
    action(
        "selection",
        "pin",
        "Hold files as they were at a time: a past one, or now to keep their current version",
        &[PATTERN, VERSION_TIME],
        Handler::Engine(perform::pin),
    ),
    action(
        "selection",
        "free",
        "Stop holding files on this machine, freeing the space",
        &[PATTERN],
        Handler::Engine(perform::free),
    ),
    view(
        "selection",
        "times",
        "List the times at which pinning files holds something new: those of their versions",
        &[PATTERN],
        &["time", "files"],
        Handler::Engine(perform::list_pin_times),
    ),
    view(
        "selection",
        "places",
        "List the folders this machine keeps elsewhere, and why any waits",
        &[],
        &["folder", "destination", "problem"],
        Handler::Engine(perform::list_places),
    ),
    action(
        "selection",
        "place",
        "Keep a folder at another destination, such as a bigger disk, leaving a link at its place",
        &[
            FOLDER,
            required(
                "destination",
                "Where to keep it: an absolute path whose parent folder exists, outside the root and other destinations",
                Kind::Folder,
            ),
        ],
        Handler::Engine(perform::place),
    ),
    action(
        "selection",
        "unplace",
        "Bring a placed folder back into the root",
        &[FOLDER],
        Handler::Engine(perform::unplace),
    ),
    view(
        "config",
        "show",
        "Show the group's config.toml as it is now, with its version",
        &[],
        &[],
        Handler::Daemon(perform::show_config),
    ),
    view(
        "config",
        "preview",
        "Show what applying a configuration would download, free and pin here, rule by rule: the text given, or config.toml as it is now",
        &[optional(
            "text",
            "A file holding the configuration, or - for standard input",
            Kind::Document,
        )],
        &[],
        Handler::Daemon(perform::preview_config),
    ),
    action(
        "config",
        "set",
        "Replace config.toml with a text that reads, and apply it to this group",
        &[
            required(
                "text",
                "A file holding the configuration, or - for standard input",
                Kind::Document,
            ),
            optional(
                "version",
                "The version `config show` gave: refuse if config.toml changed since",
                Kind::Text,
            ),
            YES,
        ],
        Handler::Daemon(perform::set_config),
    ),
    view(
        "suggestion",
        "list",
        "List the suggestions, oldest first: the changes the rules leave to the group, with why each waits",
        &[],
        &["id", "author", "time", "reason", "changes.path"],
        Handler::Engine(perform::list_suggestions),
    ),
    action(
        "suggestion",
        "validate",
        "Validate suggestions: publish their changes, the later one winning at a path",
        &[
            SUGGESTIONS,
            optional(
                "to",
                "Publish the one file of a single suggestion at this free path instead, such as for a name some machine cannot hold",
                Kind::Path,
            ),
            YES,
        ],
        Handler::Engine(perform::validate),
    ),
    action(
        "suggestion",
        "discard",
        "Discard suggestions: the group keeps its versions, and the history keeps theirs",
        &[SUGGESTIONS, YES],
        Handler::Engine(perform::discard),
    ),
    on_machine(Action {
        columns: &["group", "download", "free", "pin"],
        ..action(
            "daemon",
            "reload",
            "Restart every group from its files, applying the edits of each group's config.toml and leaving as it is a group whose config.toml does not read; tell what they download, free and pin here",
            &[YES],
            Handler::Daemon(perform::reload),
        )
    }),
    on_machine(action(
        "daemon",
        "stop",
        "Stop the daemon",
        &[],
        Handler::Daemon(perform::stop),
    )),
    on_machine(view(
        "daemon",
        "program",
        "Show the program file the daemon runs, which an update replaces",
        &[],
        &[],
        Handler::Daemon(perform::program),
    )),
    on_machine(action(
        "daemon",
        "restart",
        "Restart the daemon onto the program now installed where it came from, unless that is the program it runs",
        &[],
        Handler::Daemon(perform::restart),
    )),
];

/// The action `noun verb`.
#[must_use]
pub fn find(noun: &str, verb: &str) -> Option<&'static Action> {
    ACTIONS
        .iter()
        .find(|action| action.noun == noun && action.verb == verb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_unique_and_their_columns_and_params_are_distinct() {
        for (index, action) in ACTIONS.iter().enumerate() {
            assert!(
                ACTIONS[..index]
                    .iter()
                    .all(|other| (other.noun, other.verb) != (action.noun, action.verb)),
                "{} is defined twice",
                action.command()
            );
            for (at, param) in action.params.iter().enumerate() {
                assert_ne!(param.name, GROUP.name);
                assert!(
                    action.params[..at]
                        .iter()
                        .all(|other| other.name != param.name)
                );
            }
            assert!(action.changes || action.params.iter().all(|param| param.kind != Kind::Bytes));
            assert!(
                action
                    .params
                    .iter()
                    .filter(|param| param.kind == Kind::Bytes)
                    .count()
                    <= 1
            );
            let asks = action.param(YES.name).is_some();
            assert!(
                !asks || action.params.iter().all(|param| param.kind != Kind::Bytes),
                "{} asks, and its confirmation page cannot repeat an upload",
                action.command()
            );
            assert!(NOUNS.iter().any(|(noun, _)| *noun == action.noun));
            let params = std::iter::once(&GROUP).chain(action.params);
            for (noun, verb) in params.filter_map(|param| param.listed_by) {
                let listing = find(noun, verb).expect("a listing action");
                assert!(!listing.changes && !listing.columns.is_empty());
            }
        }
    }

    #[test]
    fn an_action_on_a_group_engine_takes_the_group() {
        for action in ACTIONS {
            assert!(
                action.scope == Scope::Group || matches!(action.handler, Handler::Daemon(_)),
                "{} acts on a group it cannot name",
                action.command()
            );
        }
    }
}
