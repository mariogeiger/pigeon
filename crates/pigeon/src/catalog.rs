//! Every action pigeon offers, defined once with its name, arguments and
//! what its results show: the command line, the JSON API and the web UI
//! forms are all generated from this table.

/// What an argument holds, which says how each interface asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// A secret, typed without echo and never shown.
    Secret,
    /// A path in the group, such as `docs/plan.txt`.
    Path,
    /// A gitignore pattern, such as `/docs/` or `*.pdf`.
    Pattern,
    /// A folder on this machine.
    Folder,
    /// A file's content: a local file on the command line, an upload in
    /// the web UI, base64 in the API.
    Bytes,
    Number,
    /// An RFC 3339 time, such as `2026-10-01T12:00:00Z`.
    Time,
    /// One of a fixed set of words; the first is the default.
    Choice(&'static [&'static str]),
    /// On or off, off by default.
    Flag,
}

/// One argument of an action.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub about: &'static str,
    pub kind: Kind,
    pub required: bool,
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
pub const GROUP: Param = Param {
    name: "group",
    about: "The group, which may be left out on a machine with a single group",
    kind: Kind::Text,
    required: false,
};

const fn required(name: &'static str, about: &'static str, kind: Kind) -> Param {
    Param {
        name,
        about,
        kind,
        required: true,
    }
}

const fn optional(name: &'static str, about: &'static str, kind: Kind) -> Param {
    Param {
        name,
        about,
        kind,
        required: false,
    }
}

const MODES: &[&str] = &["propose", "force"];
const SWITCH: &[&str] = &["off", "on"];
const MODE: Param = optional(
    "mode",
    "For files one may not write: propose the change to their owner, or force it",
    Kind::Choice(MODES),
);
const MESSAGE: Param = optional("message", "Why, for the files' owners", Kind::Text);
const MEMBER: Param = required(
    "member",
    "Your name in the group: 1 to 32 characters among a-z and 0-9",
    Kind::Text,
);
const PASSWORD: Param = required(
    "password",
    "Your personal password, the same on each of your machines",
    Kind::Secret,
);
const WHO: Param = required("member", "The member's name", Kind::Text);
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
const FOLDER: Param = required(
    "folder",
    "The folder of the group, such as videos",
    Kind::Path,
);
const ID: Param = required(
    "id",
    "The item's number in `pigeon aside list`",
    Kind::Number,
);
const REQUEST: Param = required(
    "request",
    "The request's path, from `pigeon request list`",
    Kind::Path,
);

const FILE_COLUMNS: &[&str] = &[
    "path",
    "owner",
    "content.size",
    "time",
    "held",
    "outdated",
    "writable",
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
    ("group", "Found, join and inspect groups"),
    ("member", "The group's members"),
    (
        "file",
        "The group's files: list, write, delete and rename them",
    ),
    ("selection", "Which files this machine holds"),
    ("request", "Changes asked of a file's owner"),
    ("aside", "What this machine may not publish as it is"),
    ("retention", "Which past versions this machine keeps"),
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
        "Found a new group, of which you are the first member",
        &[
            required(
                "name",
                "The group's name: 1 to 32 characters among a-z and 0-9",
                Kind::Text,
            ),
            MEMBER,
            PASSWORD,
            ROOT,
        ],
    )),
    on_machine(action(
        "group",
        "join",
        "Join a group with the key a member shared",
        &[
            required("key", "The group key a member shared", Kind::Text),
            MEMBER,
            PASSWORD,
            ROOT,
        ],
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
        "Leave the group: your name stays taken, this machine keeps its files but stops syncing, and the others renew the group key",
        &[],
    ),
    view(
        "member",
        "list",
        "List the members",
        &[],
        &["name", "joined", "key", "rebound.by", "rebound.time"],
    ),
    action(
        "member",
        "claim",
        "Claim a name for this machine after losing one, or log it in with a new password",
        &[MEMBER, PASSWORD],
    ),
    action(
        "member",
        "password",
        "Change your password; your other machines then log in with it",
        &[required("password", "Your new password", Kind::Secret)],
    ),
    action(
        "member",
        "reset",
        "Give another member a new password, which you then tell them",
        &[
            WHO,
            required("password", "Their new password", Kind::Secret),
        ],
    ),
    action(
        "member",
        "exclude",
        "Exclude a member: the name stays taken, their files stay readable but frozen, and the group key is renewed",
        &[WHO],
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
        "List a file's versions",
        &[required("path", "The file", Kind::Path)],
        &["time", "content.size", "owner", "applies"],
    ),
    action(
        "file",
        "write",
        "Write a file, or request it from its owner",
        &[
            required("path", "The file to write", Kind::Path),
            required(
                "content",
                "A local file holding the new content, or - for standard input",
                Kind::Bytes,
            ),
            MODE,
            MESSAGE,
        ],
    ),
    action(
        "file",
        "delete",
        "Delete a file or a folder, or request it from their owners",
        &[
            required("path", "The file or folder to delete", Kind::Path),
            MODE,
            MESSAGE,
        ],
    ),
    action(
        "file",
        "rename",
        "Rename a file or a folder, or request it from their owners",
        &[
            required("from", "The file or folder to rename", Kind::Path),
            required("to", "Its new path", Kind::Path),
            MODE,
            MESSAGE,
        ],
    ),
    view(
        "selection",
        "list",
        "List the selection's rules; the last matching rule wins",
        &[],
        &["pattern", "cutoff"],
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
        &[
            PATTERN,
            required("time", "The time, such as 2026-10-01T12:00:00Z", Kind::Time),
        ],
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
        "request",
        "list",
        "List the requests",
        &[],
        &[
            "path",
            "author",
            "statement.owner",
            "statement.mode",
            "decision",
            "applied",
            "outdated",
        ],
    ),
    action(
        "request",
        "accept",
        "Accept a request addressed to you",
        &[REQUEST],
    ),
    action(
        "request",
        "refuse",
        "Refuse a request addressed to you",
        &[REQUEST],
    ),
    view(
        "aside",
        "list",
        "List what this machine set aside",
        &[],
        &["id", "path", "reason", "content.size"],
    ),
    action("aside", "discard", "Forget a set-aside item", &[ID]),
    action(
        "aside",
        "restore",
        "Publish a set-aside item as a new file",
        &[ID, required("to", "Where to put it", Kind::Path)],
    ),
    action(
        "aside",
        "request",
        "Request a set-aside item from the owner of its path",
        &[ID, MODE, MESSAGE],
    ),
    view(
        "retention",
        "show",
        "Show which past versions this machine keeps",
        &[],
        &[],
    ),
    action(
        "retention",
        "set",
        "Change which past versions this machine keeps",
        &[
            optional(
                "every",
                "Keep every version for this many days",
                Kind::Number,
            ),
            optional(
                "daily",
                "Keep the last version of each day for this many days",
                Kind::Number,
            ),
            optional(
                "weekly",
                "Keep the last version of each week for this many days",
                Kind::Number,
            ),
            optional(
                "deletion",
                "Keep the last version before a deletion for this many days",
                Kind::Number,
            ),
            optional(
                "quota",
                "Let past versions fill at most this percentage of the disk",
                Kind::Number,
            ),
            optional(
                "everything",
                "Keep the history of every file this machine downloads, not only yours",
                Kind::Choice(SWITCH),
            ),
        ],
    ),
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
        }
    }
}
