//! Tests of upgrading a data directory an older pigeon wrote: every setting
//! reaches today's files, the old ones go, and upgrading again does
//! nothing.

use pigeon_core::clock::Stamp;
use pigeon_core::identity::MachineCert;
use pigeon_core::path::GroupPath;
use pigeon_core::selection::Cutoff;
use serde_json::json;

use super::*;

/// Writes the data directory an older pigeon would have, with the settings
/// `settings`, and returns its machine key.
fn old_layout(
    data: &DataDir,
    key: &GroupKey,
    renewal: Renewal,
    settings: &[(&str, serde_json::Value)],
) -> SecretKey {
    let machine = SecretKey::generate();
    let member = MemberName::parse("mario").unwrap();
    let cert = MachineCert::derive(&key.group, member.clone(), machine.public());
    let config = json!({
        "key": key,
        "renewal": renewal,
        "member": member,
        "root": std::env::temp_dir().join("cheapmo"),
        "cert": cert,
    });
    std::fs::create_dir_all(data.path()).unwrap();
    std::fs::write(data.path().join("config.json"), config.to_string()).unwrap();
    std::fs::write(data.path().join("machine.key"), machine.to_bytes()).unwrap();
    let state = State::open(&data.state_path()).unwrap();
    let transaction = state.database().begin_write().unwrap();
    {
        let mut table = transaction.open_table(SETTINGS).unwrap();
        for (name, value) in settings {
            table.insert(*name, value.to_string().as_bytes()).unwrap();
        }
    }
    transaction.commit().unwrap();
    machine
}

#[test]
fn an_old_data_directory_upgrades_once_and_keeps_every_setting() {
    let dir = tempfile::tempdir().unwrap();
    let data = DataDir::new(dir.path().join("cheapmo"));
    let key = GroupKey::generate(MemberName::parse("cheapmo").unwrap(), Vec::new());
    let renewal = Renewal {
        after: Stamp {
            time: (1_790_856_000 << 32) + 3,
            machine: SecretKey::generate().public(),
        },
        by: SecretKey::generate().public(),
    };
    let destination = std::env::temp_dir().join("disk").join("videos");
    let place = json!([{ "folder": "videos", "destination": destination }]);
    let rules = json!([
        { "pattern": "+mario/", "cutoff": "PlusInfinity" },
        { "pattern": "/report/", "cutoff": { "At": 7 } },
    ]);
    let seconds = json!({ "every": 2 * DAY, "daily": 7 * DAY, "quota_percent": 5 });
    let machine = old_layout(
        &data,
        &key,
        renewal,
        &[
            ("selection", rules),
            ("retention", seconds),
            ("places", place.clone()),
            ("placed", place),
        ],
    );
    assert!(upgrade(&data).unwrap());
    for old in ["config.json", "machine.key"] {
        assert!(!data.path().join(old).exists(), "{old} is gone");
    }
    let config = data.load_config(0).unwrap();
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
    let secrets = data.secrets().unwrap();
    assert_eq!(secrets.machine.public(), machine.public());
    assert_eq!(secrets.key, Some(key));
    assert_eq!(secrets.renewal, Some(renewal));
    let state = State::open(&data.state_path()).unwrap();
    assert_eq!(state.placed().unwrap(), config.places);
    assert_eq!(
        setting::<serde_json::Value>(&state, "places").unwrap(),
        None
    );
    drop(state);
    assert!(!upgrade(&data).unwrap());
}
