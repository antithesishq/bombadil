/// This program renders the `bombadil` command tree as Markdown.
/// Used when generating the manual.
use std::fmt::Write;

use anyhow::{Result, anyhow};
use bombadil_cli::Cli;
use clap::{Arg, Command, CommandFactory, Parser};

#[derive(Parser)]
#[command(name = "cli-reference")]
struct Args {
    #[arg(long = "driver")]
    driver: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut root = Cli::command();
    root.build();
    let driver = root
        .get_subcommands()
        .find(|command| command.get_name() == args.driver)
        .ok_or_else(|| anyhow!("no such driver: {}", args.driver))?;
    let mut output = String::new();
    render_commands(
        &mut output,
        &[root.get_name(), driver.get_name()],
        driver,
    )?;
    print!("{output}");
    Ok(())
}

fn render_commands(
    output: &mut String,
    path: &[&str],
    parent: &Command,
) -> Result<()> {
    for command in parent.get_subcommands() {
        if command.is_hide_set() || command.get_name() == "help" {
            continue;
        }
        let mut path = path.to_vec();
        path.push(command.get_name());
        if command.has_subcommands() {
            render_commands(output, &path, command)?;
        } else {
            render_command(output, &path, command)?;
        }
    }
    Ok(())
}

fn render_command(
    output: &mut String,
    path: &[&str],
    command: &Command,
) -> Result<()> {
    let id = path[1..].join("-");
    let arguments: Vec<&Arg> = command
        .get_positionals()
        .filter(|arg| !arg.is_hide_set())
        .collect();
    let options: Vec<&Arg> = command
        .get_arguments()
        .filter(|arg| !arg.is_positional() && !arg.is_hide_set())
        .collect();

    writeln!(output, "### {}\n", path.join(" "))?;
    if let Some(about) = command.get_long_about().or(command.get_about()) {
        writeln!(output, "{}\n", punctuate(&rejoin_lines(&about.to_string())))?;
    }

    let mut usage: Vec<String> =
        path.iter().map(|segment| format!("`{segment}`")).collect();
    if !options.is_empty() {
        usage.push(format!("[`[OPTIONS]`](#options-{id})"));
    }
    for arg in &arguments {
        usage.push(format!("[`{}`](#arguments-{id})", positional_name(arg)));
    }
    writeln!(output, "{}\n", usage.join(" "))?;

    if !arguments.is_empty() {
        writeln!(output, "#### Arguments {{#arguments-{id}}}\n")?;
        for arg in &arguments {
            render_definition(output, &positional_name(arg), arg)?;
        }
    }

    if !options.is_empty() {
        writeln!(output, "#### Options {{#options-{id}}}\n")?;
        for arg in &options {
            render_definition(output, &option_name(arg), arg)?;
        }
    }

    if let Some(after_help) = command.get_after_help() {
        let after_help = after_help.to_string();
        let examples =
            after_help.trim().strip_prefix("Examples:").ok_or_else(|| {
                anyhow!(
                    "after_help of `{}` must start with \"Examples:\"",
                    path.join(" ")
                )
            })?;
        writeln!(output, "#### Examples {{#examples-{id}}}\n")?;
        render_examples(output, &dedent(examples))
            .map_err(|error| anyhow!("{}: {error}", path.join(" ")))?;
    }
    Ok(())
}

/// Examples are listed with a #-prefixed comment and then a code snippet. Multiple such examples
/// are contained in the examples string. We format them as separate paragraphs and code blocks.
fn render_examples(output: &mut String, examples: &str) -> Result<()> {
    for example in examples.split("\n\n") {
        let lines: Vec<&str> = example.lines().collect();
        let comment_count = lines
            .iter()
            .take_while(|line| line.starts_with('#'))
            .count();
        let (comment, code) = lines.split_at(comment_count);
        if code.is_empty() {
            return Err(anyhow!("example without code: {example:?}"));
        }
        if !comment.is_empty() {
            let comment = comment
                .iter()
                .map(|line| line.trim_start_matches('#').trim())
                .collect::<Vec<_>>()
                .join(" ");
            let comment = comment.trim_end_matches(['.', ':']);
            writeln!(output, "{}:\n", rejoin_lines(comment))?;
        }
        writeln!(output, "```bash\n{}\n```\n", code.join("\n"))?;
    }
    Ok(())
}

/// Removes surrounding blank lines and shared indentation level spaces.
fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .collect();
    let lines = match lines.iter().rposition(|line| !line.trim().is_empty()) {
        Some(last) => &lines[..=last],
        None => &[][..],
    };
    let indentation = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| line.get(indentation..).unwrap_or("").trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn value_name(arg: &Arg) -> String {
    match arg.get_value_names() {
        Some(names) if !names.is_empty() => names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
            .join(" "),
        _ => arg.get_id().as_str().to_uppercase(),
    }
}

fn positional_name(arg: &Arg) -> String {
    let name = value_name(arg);
    let name = if arg.is_required_set() {
        format!("<{name}>")
    } else {
        format!("[{name}]")
    };
    let multiple = arg
        .get_num_args()
        .is_some_and(|range| range.max_values() > 1);
    if multiple { format!("{name}...") } else { name }
}

fn option_name(arg: &Arg) -> String {
    let mut names = Vec::new();
    if let Some(short) = arg.get_short() {
        names.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        names.push(format!("--{long}"));
    }
    let name = names.join(", ");
    if arg.get_action().takes_values() {
        format!("{name} <{}>", value_name(arg))
    } else {
        name
    }
}

fn render_definition(output: &mut String, term: &str, arg: &Arg) -> Result<()> {
    let mut paragraphs = help(arg);
    if let Some(default) = default(arg) {
        paragraphs.push(format!("Default: `{default}`"));
    }
    writeln!(output, "`{term}`")?;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        let marker = if index == 0 { ":   " } else { "    " };
        writeln!(output, "{marker}{paragraph}\n")?;
    }
    if paragraphs.is_empty() {
        writeln!(output, ":   \n")?;
    }
    Ok(())
}

fn help(arg: &Arg) -> Vec<String> {
    let Some(help) = arg.get_long_help().or(arg.get_help()) else {
        return Vec::new();
    };
    help.to_string()
        .split("\n\n")
        .map(|paragraph| punctuate(&rejoin_lines(paragraph)))
        .filter(|paragraph| !paragraph.is_empty())
        .collect()
}

fn default(arg: &Arg) -> Option<String> {
    if !arg.get_action().takes_values() {
        return None;
    }
    let values: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    (!values.is_empty()).then(|| values.join(","))
}

/// End the text with a period unless it already has trailing
/// punctuation (clap strips these away automatically).
fn punctuate(text: &str) -> String {
    if text.is_empty() || text.ends_with(['.', '!', '?', ':']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

fn rejoin_lines(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
