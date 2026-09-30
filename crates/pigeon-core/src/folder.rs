//! Folder rules: which folder rule governs a path, given who the members
//! are, and which names its `@<name>` folders claim.

use crate::name::MemberName;
use crate::path::GroupPath;

/// The rule of the folder holding a file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Folder {
    /// Inside `@<owner>`: only the owner adds files, which never freeze.
    Personal(MemberName),
    /// Anywhere else: anyone adds files, each frozen once published.
    Drop,
}

impl Folder {
    #[must_use]
    pub fn owner(&self) -> Option<&MemberName> {
        match self {
            Self::Personal(owner) => Some(owner),
            Self::Drop => None,
        }
    }
}

/// The member name a folder name `@<name>` refers to, compared without case.
#[must_use]
pub fn folder_claim(folder_name: &str) -> Option<MemberName> {
    MemberName::parse(&folder_name.strip_prefix('@')?.to_lowercase()).ok()
}

/// Classifies `path`: the first folder `@<name>` whose name is a member's
/// makes it personal to that member; every other folder is a drop folder.
/// Also returns the names claimed by the `@<name>` folders met before it.
pub fn classify(
    path: &GroupPath,
    is_member: impl Fn(&MemberName) -> bool,
) -> (Folder, Vec<MemberName>) {
    let mut claimed = Vec::new();
    let mut names = path.names();
    names.next_back();
    for name in names {
        if let Some(member) = folder_claim(name) {
            if is_member(&member) {
                return (Folder::Personal(member), claimed);
            }
            claimed.push(member);
        }
    }
    (Folder::Drop, claimed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str) -> MemberName {
        MemberName::parse(name).unwrap()
    }

    #[test]
    fn first_member_folder_decides() {
        let members = [member("mario"), member("emmy")];
        let is_member = |name: &MemberName| members.contains(name);
        let classify_text = |text: &str| classify(&GroupPath::parse(text).unwrap(), is_member);
        assert_eq!(
            classify_text("src/@mario/a").0,
            Folder::Personal(member("mario"))
        );
        assert_eq!(
            classify_text("@Mario/@emmy/a").0,
            Folder::Personal(member("mario"))
        );
        assert_eq!(
            classify_text("@types/@emmy/a"),
            (Folder::Personal(member("emmy")), vec![member("types")])
        );
        assert_eq!(
            classify_text("@types/a"),
            (Folder::Drop, vec![member("types")])
        );
        assert_eq!(classify_text("@mario").0, Folder::Drop);
        assert_eq!(classify_text("@-x/a"), (Folder::Drop, vec![]));
    }
}
