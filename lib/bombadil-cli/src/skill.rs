use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::builder::PossibleValuesParser;
use serde::Serialize;

#[derive(clap::Subcommand)]
pub enum Command {
    /// Install Bombadil's agent skills into the agents found on this machine
    ///
    /// Without --agent, the skills go into the skill directory of each agent
    /// whose configuration directory exists. If no agent is found, they go
    /// into the shared .agents/skills directory that most agents read.
    Setup {
        #[command(flatten)]
        scope: Scope,
        /// Use only AGENT (repeatable)
        #[arg(long = "agent", value_name = "AGENT", value_enum)]
        agents: Vec<Agent>,
        /// Report what would change without changing anything
        #[arg(long)]
        dry_run: bool,
        /// Replace skills of the same name that Bombadil did not install
        #[arg(long)]
        force: bool,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Report whether Bombadil's agent skills are installed and current
    Status {
        #[command(flatten)]
        scope: Scope,
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove the agent skills that Bombadil installed
    ///
    /// Without --agent, removes them from every skill directory Bombadil
    /// knows, including those of agents that are no longer installed.
    Remove {
        #[command(flatten)]
        scope: Scope,
        /// Use only AGENT (repeatable)
        #[arg(long = "agent", value_name = "AGENT", value_enum)]
        agents: Vec<Agent>,
        /// Report what would change without changing anything
        #[arg(long)]
        dry_run: bool,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the agent guide for this version of Bombadil
    Show {
        #[arg(
            default_value = "finding-bugs",
            value_parser = PossibleValuesParser::new(SKILLS.iter().map(|s| s.topic)),
        )]
        topic: String,
    },
}

#[derive(clap::Args)]
pub struct Scope {
    /// Use the current directory instead of your home directory
    #[arg(long)]
    project: bool,
}

#[derive(Clone, Copy, PartialEq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Claude,
    Codex,
    Copilot,
    Cursor,
    Gemini,
    Opencode,
    Pi,
}

impl Agent {
    const ALL: [Agent; 7] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Copilot,
        Agent::Cursor,
        Agent::Gemini,
        Agent::Opencode,
        Agent::Pi,
    ];

    /// The skill directory the agent reads, relative to the scope's base.
    fn skill_dir(self) -> &'static str {
        match self {
            Agent::Claude => ".claude/skills",
            _ => ".agents/skills",
        }
    }

    /// The directory, relative to the home directory, whose presence shows
    /// that the agent is installed.
    fn config_dir(self) -> &'static str {
        match self {
            Agent::Claude => ".claude",
            Agent::Codex => ".codex",
            Agent::Copilot => ".copilot",
            Agent::Cursor => ".cursor",
            Agent::Gemini => ".gemini",
            Agent::Opencode => ".config/opencode",
            Agent::Pi => ".pi",
        }
    }
}

/// A skill Bombadil ships: the guide that `bombadil skill show TOPIC` prints,
/// and the pointer skill that `setup` installs as `NAME/SKILL.md`. The pointer
/// tells the agent to read the guide, so the guide matches the installed binary.
struct Skill {
    topic: &'static str,
    name: &'static str,
    guide: &'static str,
    stub: &'static str,
}

macro_rules! skill {
    ($topic:literal, $name:literal) => {
        Skill {
            topic: $topic,
            name: $name,
            guide: include_str!(concat!("skills/", $topic, ".md")),
            stub: include_str!(concat!(
                "../../../.agents/skills/",
                $name,
                "/SKILL.md"
            )),
        }
    };
}

const SKILLS: &[Skill] = &[
    skill!("finding-bugs", "finding-bugs-with-bombadil"),
    skill!("investigating-failures", "investigating-bombadil-failures"),
    skill!("writing-specifications", "writing-bombadil-specifications"),
    skill!(
        "reviewing-specifications",
        "reviewing-bombadil-specifications"
    ),
];

/// Every pointer skill carries this metadata key. Its presence marks a file
/// that Bombadil may update or remove.
const MARKER: &str = "bombadil-stub:";

const SKILL_DIRS: [&str; 2] = [".agents/skills", ".claude/skills"];

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum State {
    Current,
    Outdated,
    Missing,
    Foreign,
    Unreadable,
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Action {
    Install,
    Update,
    Replace,
    Remove,
    Conflict,
    Skip,
}

#[derive(Serialize)]
struct Found {
    agent: Agent,
    found: bool,
}

#[derive(Serialize)]
struct Item {
    path: PathBuf,
    /// The agents that read this file.
    agents: Vec<Agent>,
    state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<Action>,
    #[serde(skip)]
    expected: &'static str,
}

#[derive(Serialize)]
struct Report {
    dry_run: bool,
    agents: Vec<Found>,
    skills: Vec<Item>,
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Show { topic } => show(&topic),
        Command::Status { scope, json } => {
            let report = Report {
                dry_run: false,
                agents: found(),
                skills: items(&base(&scope)?, &SKILL_DIRS),
            };
            print(&report, json)
        }
        Command::Setup {
            scope,
            agents,
            dry_run,
            force,
            json,
        } => setup(&scope, agents, dry_run, force, json),
        Command::Remove {
            scope,
            agents,
            dry_run,
            json,
        } => remove(&scope, &agents, dry_run, json),
    }
}

fn show(topic: &str) -> Result<()> {
    let skill = SKILLS
        .iter()
        .find(|s| s.topic == topic)
        .expect("clap accepts only known topics");
    if let Err(error) = io::stdout().lock().write_all(skill.guide.as_bytes())
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        return Err(error.into());
    }
    Ok(())
}

fn setup(
    scope: &Scope,
    agents: Vec<Agent>,
    dry_run: bool,
    force: bool,
    json: bool,
) -> Result<()> {
    let found = found();
    let selected = if agents.is_empty() {
        found.iter().filter(|f| f.found).map(|f| f.agent).collect()
    } else {
        agents
    };
    let dirs = if selected.is_empty() {
        vec![Agent::Codex.skill_dir()]
    } else {
        dirs_for(&selected)
    };
    let mut skills = items(&base(scope)?, &dirs);
    let mut conflicts = 0;
    for item in &mut skills {
        item.action = match item.state {
            State::Current => None,
            State::Missing => Some(Action::Install),
            State::Outdated => Some(Action::Update),
            State::Foreign | State::Unreadable if force => {
                Some(Action::Replace)
            }
            State::Foreign | State::Unreadable => {
                conflicts += 1;
                Some(Action::Conflict)
            }
        };
        let writes = matches!(
            item.action,
            Some(Action::Install | Action::Update | Action::Replace)
        );
        if writes && !dry_run {
            let dir = item.path.parent().expect("skill file has a parent");
            fs::create_dir_all(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
            fs::write(&item.path, item.expected)
                .with_context(|| format!("writing {}", item.path.display()))?;
        }
    }
    print(
        &Report {
            dry_run,
            agents: found,
            skills,
        },
        json,
    )?;
    if conflicts > 0 {
        bail!(
            "{conflicts} skill(s) were not installed by Bombadil and were \
             left unchanged; pass --force to replace them"
        );
    }
    Ok(())
}

fn remove(
    scope: &Scope,
    agents: &[Agent],
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let dirs = if agents.is_empty() {
        SKILL_DIRS.to_vec()
    } else {
        dirs_for(agents)
    };
    let mut skills = items(&base(scope)?, &dirs);
    for item in &mut skills {
        item.action = match item.state {
            State::Current | State::Outdated => Some(Action::Remove),
            State::Foreign | State::Unreadable => Some(Action::Skip),
            State::Missing => None,
        };
        if item.action == Some(Action::Remove) && !dry_run {
            fs::remove_file(&item.path)
                .with_context(|| format!("removing {}", item.path.display()))?;
            // Keep the directory if anything else was put in it.
            let _ = fs::remove_dir(
                item.path.parent().expect("skill file has a parent"),
            );
        }
    }
    print(
        &Report {
            dry_run,
            agents: found(),
            skills,
        },
        json,
    )
}

fn print(report: &Report, json: bool) -> Result<()> {
    let mut out = Vec::new();
    if json {
        serde_json::to_writer_pretty(&mut out, report)?;
        writeln!(out)?;
    } else {
        write_text(&mut out, report)?;
    }
    if let Err(error) = io::stdout().lock().write_all(&out)
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        return Err(error.into());
    }
    Ok(())
}

fn write_text(out: &mut Vec<u8>, report: &Report) -> io::Result<()> {
    let found: Vec<&str> = report
        .agents
        .iter()
        .filter(|f| f.found)
        .map(|f| agent_name(f.agent))
        .collect();
    writeln!(
        out,
        "agents found: {}",
        if found.is_empty() {
            "none".to_string()
        } else {
            found.join(", ")
        }
    )?;
    for item in &report.skills {
        let label = match item.action {
            Some(action @ (Action::Conflict | Action::Skip)) => {
                action_name(action).to_string()
            }
            Some(action) if report.dry_run => {
                format!("would {}", action_name(action))
            }
            Some(action) => action_name(action).to_string(),
            None => state_name(item.state).to_string(),
        };
        writeln!(out, "{label:<14}  {}", item.path.display())?;
    }
    Ok(())
}

fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Copilot => "copilot",
        Agent::Cursor => "cursor",
        Agent::Gemini => "gemini",
        Agent::Opencode => "opencode",
        Agent::Pi => "pi",
    }
}

fn action_name(action: Action) -> &'static str {
    match action {
        Action::Install => "install",
        Action::Update => "update",
        Action::Replace => "replace",
        Action::Remove => "remove",
        Action::Conflict => "conflict",
        Action::Skip => "skip",
    }
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Current => "current",
        State::Outdated => "outdated",
        State::Missing => "missing",
        State::Foreign => "foreign",
        State::Unreadable => "unreadable",
    }
}

/// Agents are found by their configuration directory in the home directory,
/// whatever the scope.
fn found() -> Vec<Found> {
    let home = env::var_os("HOME").map(PathBuf::from);
    Agent::ALL
        .into_iter()
        .map(|agent| Found {
            agent,
            found: home
                .as_ref()
                .is_some_and(|home| home.join(agent.config_dir()).is_dir()),
        })
        .collect()
}

fn dirs_for(agents: &[Agent]) -> Vec<&'static str> {
    SKILL_DIRS
        .into_iter()
        .filter(|dir| agents.iter().any(|a| a.skill_dir() == *dir))
        .collect()
}

fn base(scope: &Scope) -> Result<PathBuf> {
    if scope.project {
        env::current_dir().context("reading the current directory")
    } else {
        Ok(PathBuf::from(
            env::var_os("HOME").context("HOME is not set")?,
        ))
    }
}

fn items(base: &Path, dirs: &[&str]) -> Vec<Item> {
    let mut items = Vec::new();
    for dir in dirs {
        for skill in SKILLS {
            let expected = skill.stub;
            let path = base.join(dir).join(skill.name).join("SKILL.md");
            items.push(Item {
                state: state(&path, expected),
                path,
                agents: Agent::ALL
                    .into_iter()
                    .filter(|a| a.skill_dir() == *dir)
                    .collect(),
                action: None,
                expected,
            });
        }
    }
    items
}

fn state(path: &Path, expected: &str) -> State {
    match fs::read_to_string(path) {
        Ok(content) if content == expected => State::Current,
        Ok(content) if content.contains(MARKER) => State::Outdated,
        Ok(_) => State::Foreign,
        // A directory without SKILL.md holds someone else's files.
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && !path.parent().is_some_and(Path::exists) =>
        {
            State::Missing
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => State::Foreign,
        Err(_) => State::Unreadable,
    }
}
