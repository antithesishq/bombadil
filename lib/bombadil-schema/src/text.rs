use std::time::SystemTime;

use crate::markup::{Inline, Markup};

pub fn markup_to_text(markup: &Markup, test_start: SystemTime) -> String {
    let mut output = String::new();
    render_markup(&mut output, markup, test_start);
    output
}

fn render_markup(output: &mut String, markup: &Markup, test_start: SystemTime) {
    match markup {
        Markup::Span(inlines) => {
            for inline in inlines {
                render_inline(output, inline, test_start);
            }
        }
        Markup::CodeBlock(code) => {
            output.push_str(code);
        }
        Markup::Snapshots(snapshots) => {
            for (index, snapshot) in snapshots.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                output.push_str(&snapshot.name);
                output.push_str(" = ");
                output
                    .push_str(&serde_json::to_string(&snapshot.value).unwrap());
            }
        }
        Markup::Stack(items) => {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push_str("\n\n");
                }
                render_markup(output, item, test_start);
            }
        }
        Markup::Join(items) => {
            render_join(output, items, test_start);
        }
        Markup::Comma => {
            // Commas are handled by Join logic, never rendered directly
        }
    }
}

fn render_join(output: &mut String, items: &[Markup], test_start: SystemTime) {
    let items = flatten_joins(items);

    let mut previous_non_comma_index: Option<usize> = None;
    let mut next_separator_has_comma = false;

    for (index, item) in items.iter().enumerate() {
        if matches!(item, Markup::Comma) {
            next_separator_has_comma = true;
            continue;
        }

        if let Some(previous_index) = previous_non_comma_index {
            let previous_inline = is_inline(&items[previous_index]);
            let current_inline = is_inline(item);

            let separator = match (previous_inline, current_inline) {
                (true, true) => {
                    if next_separator_has_comma {
                        ", "
                    } else {
                        " "
                    }
                }
                (true, false) => ":\n\n",
                (false, _) => "\n\n",
            };

            output.push_str(separator);
            next_separator_has_comma = false;
        }

        render_markup(output, item, test_start);
        previous_non_comma_index = Some(index);
    }
}

fn flatten_joins(items: &[Markup]) -> Vec<Markup> {
    let mut result = Vec::new();
    for item in items {
        if let Markup::Join(nested_items) = item {
            result.extend(flatten_joins(nested_items));
        } else {
            result.push(item.clone());
        }
    }
    result
}

fn is_inline(markup: &Markup) -> bool {
    match markup {
        Markup::Span(_) => true,
        Markup::CodeBlock(_) => false,
        Markup::Snapshots(items) => {
            items.iter().all(|item| is_json_inline(&item.value))
        }
        Markup::Stack(_) => false,
        Markup::Join(items) => items.iter().all(is_inline),
        Markup::Comma => true,
    }
}

fn is_json_inline(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => items.is_empty(),
        serde_json::Value::Object(map) => map.is_empty(),
        _ => true,
    }
}

fn render_inline(output: &mut String, inline: &Inline, test_start: SystemTime) {
    match inline {
        Inline::Text(text) => output.push_str(text),
        Inline::Code(code) => output.push_str(code),
        Inline::Time(time) => {
            output.push_str(&format_duration(*time, test_start));
        }
        Inline::Keyword(keyword) => output.push_str(keyword),
    }
}

fn format_duration(time: SystemTime, test_start: SystemTime) -> String {
    let duration = time
        .duration_since(test_start)
        .unwrap_or(std::time::Duration::ZERO);

    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;

    format!("{:02}:{:02}", minutes, seconds)
}
