//! Tests of upgrading the files an older pigeon wrote: every setting
//! reaches today's files, a certificate the name does not derive stays, a
//! configuration in the data folder moves to the configuration's, the old
//! files go, and upgrading again does nothing.

use pigeon_core::clock::Stamp;
use pigeon_core::identity::{MachineCert, member_key};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::Cutoff;
use serde_json::json;

use super::*;
use crate::group_dirs::write_private;

/// Writes the group's files as an older pigeon would have, with mario's
/// certificate by `member` and the settings `settings`, and returns its
/// machine key and the certificate.
fn old_layout(
    group: &GroupDirs,
    key: &GroupKey,
    member: &SecretKey,
    renewal: Renewal,
    settings: &[(&str, serde_json::Value)],
) -> (SecretKey, MachineCert) {
    let machine = SecretKey::generate();
    let name = MemberName::parse("mario").unwrap();
    let cert = MachineCert::issue(&key.group, name.clone(), member, machine.public());
    let config = json!({
        "key": key,
        "renewal": renewal,
        "member": name,
        "root": std::env::temp_dir().join("cheapmo"),
        "cert": cert,
    });
    std::fs::create_dir_all(group.data()).unwrap();
    std::fs::write(group.data().join("config.json"), config.to_string()).unwrap();
    std::fs::write(group.data().join("machine.key"), machine.to_bytes()).unwrap();
    let state = State::open(&group.state_path()).unwrap();
    let transaction = state.database().begin_write().unwrap();
    {
        let mut table = transaction.open_table(SETTINGS).unwrap();
        for (name, value) in settings {
            table.insert(*name, value.to_string().as_bytes()).unwrap();
        }
    }
    transaction.commit().unwrap();
    (machine, cert)
}

fn renewal() -> Renewal {
    Renewal {
        after: Stamp {
            time: (1_790_856_000 << 32) + 3,
            machine: SecretKey::generate().public(),
        },
        by: SecretKey::generate().public(),
    }
}

#[test]
fn an_old_data_directory_upgrades_once_and_keeps_every_setting() {
    let dir = tempfile::tempdir().unwrap();
    let group = GroupDirs::new(dir.path().join("config"), dir.path().join("data"));
    let key = GroupKey::generate(MemberName::parse("cheapmo").unwrap(), Vec::new());
    let renewal = renewal();
    let random_member = SecretKey::generate();
    let destination = std::env::temp_dir().join("disk").join("videos");
    let place = json!([{ "folder": "videos", "destination": destination }]);
    let rules = json!([
        { "pattern": "+mario/", "cutoff": "PlusInfinity" },
        { "pattern": "/report/", "cutoff": { "At": 7 } },
    ]);
    let seconds = json!({ "every": 2 * DAY, "daily": 7 * DAY, "quota_percent": 5 });
    let (machine, cert) = old_layout(
        &group,
        &key,
        &random_member,
        renewal,
        &[
            ("selection", rules),
            ("retention", seconds),
            ("places", place.clone()),
            ("placed", place),
        ],
    );
    assert!(upgrade(&group).unwrap());
    assert!(group.config_path().is_file());
    for old in ["config.json", "machine.key", "config.toml"] {
        assert!(!group.data().join(old).exists(), "{old} is gone");
    }
    let config = group.load_config().unwrap();
    assert_eq!(config.member.as_str(), "mario");
    let rules: Vec<String> = config.selection.rules().map(Rule::to_string).collect();
    assert_eq!(rules[0], "follow +mario/");
    assert_eq!(
        config.selection.rules().nth(1).unwrap().cutoff,
        Cutoff::At(7)
    );
    assert_eq!(
        config.retention,
        Retention {
            every: 2,
            daily: 7,
            quota_percent: 5,
            ..Retention::default()
        }
    );
    let videos = GroupPath::parse("videos").unwrap();
    assert_eq!(config.places.get(&videos), Some(destination.as_path()));
    let secrets = group.secrets().unwrap();
    assert_eq!(secrets.machine.public(), machine.public());
    assert_eq!(secrets.key.as_ref(), Some(&key));
    assert_eq!(secrets.renewal, Some(renewal));
    assert_eq!(secrets.cert, Some(cert.clone()));
    let mario = MemberName::parse("mario").unwrap();
    assert_eq!(secrets.cert_of(&key.group, &mario), Ok(cert));
    let carol = MemberName::parse("carol").unwrap();
    let derived = MachineCert::derive(&key.group, carol.clone(), machine.public());
    assert_eq!(secrets.cert_of(&key.group, &carol), Ok(derived));
    let state = State::open(&group.state_path()).unwrap();
    assert_eq!(state.placed().unwrap(), config.places);
    assert_eq!(
        setting::<serde_json::Value>(&state, "places").unwrap(),
        None
    );
    drop(state);
    assert!(!upgrade(&group).unwrap());
}

#[test]
fn a_certificate_the_name_derives_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let group = GroupDirs::new(dir.path().join("cheapmo"), dir.path().join("cheapmo"));
    let key = GroupKey::generate(MemberName::parse("cheapmo").unwrap(), Vec::new());
    let mario = MemberName::parse("mario").unwrap();
    let (_, cert) = old_layout(
        &group,
        &key,
        &member_key(&key.group, &mario),
        renewal(),
        &[],
    );
    assert!(upgrade(&group).unwrap());
    let secrets = group.secrets().unwrap();
    assert_eq!(secrets.cert, None);
    assert_eq!(secrets.cert_of(&key.group, &mario), Ok(cert));
}

#[test]
fn a_configuration_in_the_data_folder_moves_to_the_configurations() {
    let dir = tempfile::tempdir().unwrap();
    let group = GroupDirs::new(dir.path().join("config"), dir.path().join("data"));
    let old = group.data().join("config.toml");
    write_private(&old, b"member = \"mario\"\n").unwrap();
    assert!(upgrade(&group).unwrap());
    let moved = std::fs::read_to_string(group.config_path()).unwrap();
    assert_eq!(moved, "member = \"mario\"\n");
    assert!(!old.exists());
    assert!(!upgrade(&group).unwrap());
}
