use bombadil_cli::Cli;
use clap::{CommandFactory, Parser};
use std::{collections::BTreeSet, process::Command};

fn show(topic: Option<&str>) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_bombadil"))
        .args(["skill", "show"])
        .args(topic)
        .output()
        .unwrap();
    assert!(output.status.success(), "{topic:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// Every topic `bombadil skill show` accepts, read from the CLI definition.
fn topics() -> Vec<String> {
    Cli::command()
        .find_subcommand("skill")
        .and_then(|skill| skill.find_subcommand("show"))
        .and_then(|show| show.get_arguments().find(|a| a.get_id() == "topic"))
        .unwrap()
        .get_possible_values()
        .iter()
        .map(|value| value.get_name().to_string())
        .collect()
}

/// Checks the commands `text` tells an agent to run: whole-line examples and
/// inline `bombadil skill ...` references to other guides.
fn check_commands(source: &str, text: &str) {
    let examples = text.lines().filter(|line| line.starts_with("bombadil "));
    let references = text
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|code| code.starts_with("bombadil skill"));
    for command in examples.chain(references) {
        if cfg!(feature = "terminal")
            || !command.starts_with("bombadil terminal ")
        {
            Cli::try_parse_from(command.split_whitespace()).unwrap_or_else(
                |error| {
                    panic!("invalid command in {source}: {command}\n{error}")
                },
            );
        }
    }
}

const SKILLS_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../.agents/skills");

fn stub(name: &str) -> String {
    std::fs::read_to_string(format!("{SKILLS_DIR}/{name}/SKILL.md")).unwrap()
}

/// The pointer skills in the repository, found by their marker.
fn stubs() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(SKILLS_DIR)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| stub(name).contains("bombadil-stub:"))
        .collect();
    names.sort();
    names
}

#[test]
fn guides_and_stubs_name_valid_commands() {
    assert_eq!(show(None), show(Some("finding-bugs")));
    let mut titles = BTreeSet::new();
    for topic in topics() {
        let guide = show(Some(&topic));
        let title = guide.lines().next().unwrap().to_string();
        assert!(title.starts_with("# "), "{topic}: {title}");
        assert!(titles.insert(title), "{topic} repeats another guide");
        check_commands(&topic, &guide);
    }

    let mut routed = BTreeSet::new();
    for name in stubs() {
        let stub = stub(&name);
        assert!(stub.contains(&format!("\nname: {name}\n")), "{name}");
        check_commands(&name, &stub);
        routed.extend(
            topics().into_iter().filter(|t| {
                stub.contains(&format!("bombadil skill show {t}`"))
            }),
        );
    }
    let topics: BTreeSet<String> = topics().into_iter().collect();
    assert_eq!(routed, topics, "every topic needs a stub that loads it");
}

fn skill(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bombadil"))
        .arg("skill")
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn setup_and_remove_leave_foreign_skills_alone() {
    let stubs = stubs();
    let name = &stubs[0];
    let home = tempfile::tempdir().unwrap();
    let agents = home.path().join(".agents/skills");
    let claude = home.path().join(".claude/skills");
    // Claude reads .claude/skills; Codex reads .agents/skills.
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::create_dir_all(home.path().join(".codex")).unwrap();

    let output = skill(home.path(), &["setup", "--dry-run"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!agents.exists() && !claude.exists());

    // An outdated copy of ours and a same-named skill someone else wrote.
    let outdated = claude.join(name).join("SKILL.md");
    std::fs::create_dir_all(outdated.parent().unwrap()).unwrap();
    std::fs::write(&outdated, "metadata:\n  bombadil-stub: \"0\"\n").unwrap();
    let foreign = agents.join(name).join("SKILL.md");
    std::fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    std::fs::write(&foreign, "someone else's skill").unwrap();

    let output = skill(home.path(), &["setup"]);
    assert!(!output.status.success(), "conflict must fail: {output:?}");
    assert_eq!(
        std::fs::read_to_string(&foreign).unwrap(),
        "someone else's skill"
    );
    for dir in [&agents, &claude] {
        for name in &stubs {
            let path = dir.join(name).join("SKILL.md");
            if path != foreign {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), stub(name));
            }
        }
    }

    let output = skill(home.path(), &["setup", "--force"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(std::fs::read_to_string(&foreign).unwrap(), stub(name));

    // Removal keeps foreign skills and directories holding other files.
    std::fs::write(agents.join(name).join("notes.txt"), "mine").unwrap();
    std::fs::write(&outdated, "someone else's skill").unwrap();
    let output = skill(home.path(), &["remove"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(&outdated).unwrap(),
        "someone else's skill"
    );
    assert!(agents.join(name).join("notes.txt").exists());
    assert!(!foreign.exists());
    for other in &stubs[1..] {
        assert!(!agents.join(other).exists() && !claude.join(other).exists());
    }
}
