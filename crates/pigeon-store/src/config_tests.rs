//! Tests of `config.toml`: a configuration reads back as pigeon writes it,
//! a hand-written one is checked and completed, and pigeon never writes
//! over edits it has not read.

use pigeon_core::path::GroupPath;
use pigeon_core::selection::Cutoff;

use super::*;

fn member(name: &str) -> MemberName {
    MemberName::parse(name).unwrap()
}

fn rules(config: &Config) -> Vec<Rule> {
    config.selection.rules().cloned().collect()
}

/// The text of a configuration of mario's with the root `root` and `rest`.
fn written(root: &Path, rest: &str) -> String {
    format!(
        "member = \"mario\"\nroot = {:?}\n{rest}",
        root.to_str().unwrap()
    )
}

fn sample() -> Config {
    let root = std::env::temp_dir().join("cheapmo");
    let mut config = Config::new(member("mario"), root.clone());
    for (pattern, cutoff) in [
        ("/docs/", Cutoff::PlusInfinity),
        ("/my report/", Cutoff::At((1_790_856_000 << 32) + 3)),
        ("*.iso", Cutoff::MinusInfinity),
    ] {
        let rule = Rule {
            pattern: pattern.into(),
            cutoff,
        };
        config.selection.set(rule).unwrap();
    }
    config.retention.daily = 7;
    config.retention.everything = true;
    let videos = GroupPath::parse("videos").unwrap();
    let destination = std::env::temp_dir().join("disk").join("videos");
    config
        .places
        .set(&root, videos, destination, layout::resolved)
        .unwrap();
    config
}

#[test]
fn a_configuration_reads_back_as_pigeon_writes_it() {
    let config = sample();
    let text = config.render().unwrap();
    assert!(text.starts_with(HEADER), "{text}");
    assert!(text.contains("\"pin 2026-10-01T12:00:00."), "{text}");
    assert!(
        text.contains("[retention]\nevery = 1\ndaily = 7\n"),
        "{text}"
    );
    assert!(text.contains("[[places]]\nfolder = \"videos\"\n"), "{text}");
    let (read, respelled) = Config::parse(&text).unwrap();
    assert!(!respelled);
    assert_eq!(read.member, config.member);
    assert_eq!(read.root, config.root);
    assert_eq!(rules(&read), rules(&config));
    assert_eq!(read.retention, config.retention);
    assert_eq!(read.places, config.places);
    assert_eq!(read.render().unwrap(), text);
}

#[test]
fn a_hand_written_configuration_takes_defaults_and_refuses_pin_now() {
    let root = std::env::temp_dir().join("g");
    let (bare, respelled) = Config::parse(&written(&root, "")).unwrap();
    assert!(respelled, "an empty selection follows the personal folder");
    assert_eq!(
        rules(&bare),
        rules(&Config::new(member("mario"), root.clone()))
    );
    assert_eq!(bare.retention, Retention::default());
    let text = written(
        &root,
        "# mine\nselection = [\"pin 2026-10-01T12:00:00Z /a/\", \"free *.iso\"]\n[retention]\nquota = 3\n",
    );
    let (config, respelled) = Config::parse(&text).unwrap();
    assert!(!respelled);
    assert_eq!(rules(&config)[0].cutoff, Cutoff::At(1_790_856_000 << 32));
    assert_eq!(config.retention.quota_percent, 3);
    assert_eq!(config.retention.daily, Retention::default().daily);
    let followed = written(&root, "selection = [\"follow /a/\"]\n");
    assert!(!Config::parse(&followed).unwrap().1);
    let now = written(&root, "selection = [\"pin now /a/\"]\n");
    let reason = Config::parse(&now).unwrap_err();
    assert!(reason.contains("RFC 3339"), "{reason}");
}

#[test]
fn an_invalid_configuration_says_what_is_wrong() {
    let root = std::env::temp_dir().join("g");
    let destination = std::env::temp_dir().join("disk");
    let place = |folder: &str, at: &Path| {
        format!(
            "[[places]]\nfolder = {folder:?}\ndestination = {:?}\n",
            at.to_str().unwrap()
        )
    };
    for (text, error) in [
        (written(&root, "colour = \"red\"\n"), "colour"),
        (written(&root, "selection = [\"keep /a/\"]\n"), "keep /a/"),
        (written(&root, "[retention]\nevry = 1\n"), "evry"),
        ("member = \"mario\"\nroot = \"g\"\n".to_owned(), "absolute"),
        ("member = \"Mario!\"\nroot = \"/g\"\n".to_owned(), "member"),
        (
            written(
                &root,
                &(place("a", &destination) + &place("a/b", &destination.join("b"))),
            ),
            "places",
        ),
    ] {
        let reason = Config::parse(&text).unwrap_err();
        assert!(reason.contains(error), "{text}: {reason}");
    }
}

#[cfg(unix)]
#[test]
fn a_destination_nests_where_the_system_resolves_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&root, dir.path().join("link")).unwrap();
    let place = |folder: &str, at: &Path| {
        format!(
            "[[places]]\nfolder = {folder:?}\ndestination = {:?}\n",
            at.to_str().unwrap()
        )
    };
    for destination in [
        dir.path().join("link/docs"),
        dir.path().join("missing/../root/docs"),
    ] {
        let text = written(&root, &place("docs", &destination));
        let reason = Config::parse(&text).unwrap_err();
        assert!(reason.contains("the root"), "{reason}");
    }
    let disk = dir.path().join("disk");
    let both = place("videos", &disk) + &place("docs", &dir.path().join("x/../disk/docs"));
    let reason = Config::parse(&written(&root, &both)).unwrap_err();
    assert!(reason.contains("the destination of videos"), "{reason}");
    let apart = place("videos", &disk) + &place("docs", &dir.path().join("docs"));
    let (config, _) = Config::parse(&written(&root, &apart)).unwrap();
    assert_eq!(
        config.places.get(&GroupPath::parse("docs").unwrap()),
        Some(dir.path().join("docs").as_path())
    );
}

#[test]
fn pigeon_writes_back_what_it_completes_and_never_over_unread_edits() {
    let dir = tempfile::tempdir().unwrap();
    let group = GroupDirs::new(dir.path().join("config"), dir.path().join("data"));
    let root = std::env::temp_dir().join("g");
    write_private(&group.config_path(), written(&root, "").as_bytes()).unwrap();
    let mut file = ConfigFile::open(&group).unwrap();
    let on_disk = std::fs::read_to_string(group.config_path()).unwrap();
    assert_eq!(on_disk, file.text());
    assert!(on_disk.contains("follow +mario/"), "{on_disk}");
    let mut config = Config::clone(&file);
    config.retention.every = 2;
    file.save(config.clone()).unwrap();
    assert_eq!(group.load_config().unwrap().retention.every, 2);
    std::fs::write(
        group.config_path(),
        on_disk.replace("every = 1", "every = 9"),
    )
    .unwrap();
    let error = file.save(config).unwrap_err().to_string();
    assert!(error.contains("pigeon daemon reload"), "{error}");
    assert_eq!(group.load_config().unwrap().retention.every, 9);
    let reopened = ConfigFile::open(&group).unwrap();
    assert_eq!(reopened.retention.every, 9);
    let invalid = group.config_path();
    std::fs::write(&invalid, "member = 1\n").unwrap();
    let error = group.load_config().unwrap_err().to_string();
    assert!(error.starts_with(&invalid.display().to_string()), "{error}");
}
