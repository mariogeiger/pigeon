//! Every action pigeon offers, defined once with its name, arguments and
//! what its results show: the command line, the JSON API and the web UI
//! forms are all generated from this table.

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
    /// A file's content: a local file on the command line, an upload in
    /// the web UI, base64 in the API.
    Bytes,
    /// An RFC 3339 time, such as `2026-10-01T12:00:00Z`.
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
    "The time, such as 2026-10-01T12:00:00Z; `pigeon selection times` lists those of the versions",
    Kind::Time,
)
.listed_by("selection", "times");
const YES: Param = optional(
    "yes",
    "Apply it even where it frees space on this machine, without asking",
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
) -> Action {
    Action {
        noun,
        verb,
        about,
        scope: Scope::Group,
        params,
        changes: true,
        columns: &[],
    }
}

const fn view(
    noun: &'static str,
    verb: &'static str,
    about: &'static str,
    params: &'static [Param],
    columns: &'static [&'static str],
) -> Action {
    Action {
        noun,
        verb,
        about,
        scope: Scope::Group,
        params,
        changes: false,
        columns,
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
    )),
    on_machine(view(
        "group",
        "names",
        "Hear a group with its key, without joining it, and list the names to join under: a member's, to add a machine of theirs, or any name not taken",
        &[KEY],
        &[],
    )),
    on_machine(action(
        "group",
        "join",
        "Join a group with the key a member shared",
        &[KEY, MEMBER, ROOT],
    )),
    view(
        "group",
        "status",
        "Show how the group stands on this machine",
        &[],
        &[],
    ),
    view(
        "group",
        "key",
        "Show the group key, which admits a new member's machine",
        &[],
        &[],
    ),
    action(
        "group",
        "relay",
        "Name the relay that carries, for every machine of the group, what no direct connection can",
        &[optional(
            "url",
            "The URL `pigeon relay` printed; leave it out for iroh's public relays",
            Kind::Text,
        )],
    ),
    action(
        "group",
        "leave",
        "Leave the group on this machine: it stops syncing and forgets the group's key, secrets and state, keeping its files; your name stays a member's",
        &[],
    ),
    view(
        "member",
        "list",
        "List the members",
        &[],
        &["name", "machines", "online", "joined"],
    ),
    action(
        "member",
        "claim",
        "Claim a name for this machine after losing one",
        &[MEMBER],
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
    ),
    view(
        "file",
        "history",
        "List a file's versions, oldest first, back through the paths it moved from",
        &[required("path", "The file", Kind::Path)],
        &["time", "path", "content.size", "author"],
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
    ),
    action(
        "file",
        "delete",
        "Delete a file or a folder; their history keeps them",
        &[required("path", "The file or folder to delete", Kind::Path)],
    ),
    action(
        "file",
        "rename",
        "Rename or move a file or a folder, which keeps its history",
        &[
            required("from", "The file or folder to rename", Kind::Path),
            required("to", "Its new path", Kind::Path),
        ],
    ),
    action(
        "file",
        "restore",
        "Bring files back as they were at a past time, as new versions that undo nothing of the history",
        &[PATTERN, VERSION_TIME],
    ),
    action(
        "selection",
        "follow",
        "Keep files in sync on this machine",
        &[PATTERN],
    ),
    action(
        "selection",
        "download",
        "Download the current version of files once, or refresh it",
        &[PATTERN],
    ),
    action(
        "selection",
        "unfollow",
        "Stop following files, keeping their current version unless freed",
        &[
            PATTERN,
            optional(
                "free",
                "Remove them from this machine to free the space",
                Kind::Flag,
            ),
        ],
    ),
    action(
        "selection",
        "pin",
        "Hold files as they were at a past time",
        &[PATTERN, VERSION_TIME],
    ),
    view(
        "selection",
        "times",
        "List the times at which pinning files holds something new: those of their versions",
        &[PATTERN],
        &["time", "files"],
    ),
    view(
        "selection",
        "places",
        "List the folders this machine keeps elsewhere, and why any waits",
        &[],
        &["folder", "destination", "problem"],
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
    ),
    action(
        "selection",
        "unplace",
        "Bring a placed folder back into the root",
        &[FOLDER],
    ),
    view(
        "config",
        "show",
        "Show the group's config.toml as it is now, with its version",
        &[],
        &[],
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
    ),
    view(
        "suggestion",
        "list",
        "List the suggestions, oldest first: the changes the rules leave to the group, with why each waits",
        &[],
        &["id", "author", "time", "reason", "changes.path"],
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
        ],
    ),
    action(
        "suggestion",
        "discard",
        "Discard suggestions: the group keeps its versions, and the history keeps theirs",
        &[SUGGESTIONS],
    ),
    on_machine(Action {
        columns: &["group", "download", "free", "pin"],
        ..action(
            "daemon",
            "reload",
            "Restart every group from its files, applying the edits of each group's config.toml, unless one does not read; tell what they download, free and pin here",
            &[YES],
        )
    }),
    on_machine(action("daemon", "stop", "Stop the daemon", &[])),
    on_machine(action(
        "daemon",
        "restart",
        "Restart the daemon onto the program now installed where it came from, unless that is the program it runs",
        &[],
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
            assert!(NOUNS.iter().any(|(noun, _)| *noun == action.noun));
            let params = std::iter::once(&GROUP).chain(action.params);
            for (noun, verb) in params.filter_map(|param| param.listed_by) {
                let listing = find(noun, verb).expect("a listing action");
                assert!(!listing.changes && !listing.columns.is_empty());
            }
        }
    }
}
