use anyhow::Result;
use oxc::span::SourceType;
use scraper::{ElementRef, Html, node::Text};

use crate::instrumentation::{js::instrument_source_code, source_id::SourceId};

pub fn instrument_inline_scripts(
    source_id: SourceId,
    input: &str,
) -> Result<String> {
    let mut document = Html::parse_document(input);
    transform_inline_scripts(source_id, &mut document)?;
    Ok(document.html())
}

fn transform_inline_scripts(
    source_id: SourceId,
    document: &mut Html,
) -> Result<()> {
    let mut scripts_count = 0;

    let mut scripts = vec![];
    {
        let mut stack: Vec<ElementRef> = Vec::new();
        stack.push(document.root_element());

        while let Some(element_ref) = stack.pop() {
            let element = element_ref.value();
            if element.name.local.as_ref() == "script" {
                let script_src = element
                    .attrs
                    .iter()
                    .find(|(name, _)| name.local.as_ref() == "src")
                    .map(|(_, value)| value.to_string());

                let script_type = element
                    .attrs
                    .iter()
                    .find(|(name, _)| name.local.as_ref() == "type")
                    .map(|(_, value)| value.to_string())
                    .unwrap_or("".to_string());

                let is_inline_javascript = script_src.is_none()
                    && (script_type.is_empty()
                        || script_type == "text/javascript"
                        || script_type == "module");

                let source_type = if script_type == "module" {
                    SourceType::mjs()
                } else {
                    SourceType::cjs()
                };

                if is_inline_javascript {
                    let text_ids: Vec<_> = element_ref
                        .children()
                        .filter(|child| child.value().is_text())
                        .map(|child| child.id())
                        .collect();

                    let mut text_all = String::new();
                    for text in element_ref
                        .children()
                        .filter_map(|child| child.value().as_text())
                    {
                        text_all.push_str(text);
                    }

                    scripts.push((
                        element_ref.id(),
                        text_ids,
                        text_all,
                        source_type,
                        source_id.add(scripts_count),
                    ));
                    scripts_count += 1;
                }
            }

            for child in element_ref.child_elements() {
                stack.push(child);
            }
        }
    }

    for (script_id, text_ids, original, source_type, source_id) in scripts {
        let transformed = instrument_source_code(
            // Every inline scripts needs a unique ID.
            // TODO: fix this, we need a global counter
            source_id,
            &original,
            source_type,
        )?;

        for text_id in text_ids {
            document
                .tree
                .get_mut(text_id)
                .expect("failed to get mutable text node")
                .detach();
        }

        let mut node = document
            .tree
            .get_mut(script_id)
            .expect("failed to get mutable script element node");

        node.append(scraper::Node::Text(Text {
            text: transformed.into(),
        }));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use insta::assert_snapshot;

    #[test]
    fn test_instrument_html_inline_script_no_type() {
        let input = indoc! { r#"
        <!DOCTYPE html>
        <html>
        <body>
        <script>
        function example(a, b, c) {
            return a ? b : c;
        }
        console.log(example(true, 1, 2));
        </script>
        </body>
        </html>
        "# };

        let output = instrument_inline_scripts(SourceId(0), input).unwrap();
        assert_snapshot!(output);
    }

    #[test]
    fn test_instrument_html_inline_script_javascript_type() {
        let input = indoc! { r#"
        <!DOCTYPE html>
        <html>
        <body>
        <script type="text/javascript">
        function example(a, b, c) {
            return a ? b : c;
        }
        console.log(example(true, 1, 2));
        </script>
        </body>
        </html>
        "# };

        let output = instrument_inline_scripts(SourceId(0), input).unwrap();
        assert_snapshot!(output);
    }

    #[test]
    fn test_instrument_html_inline_script_other_type() {
        let input = indoc! { r#"
        <!DOCTYPE html>
        <html>
        <body>
        <script type="text/other">
        if (foo) {
            this_is_not_a_script();
        }
        </script>
        </body>
        </html>
        "# };

        let output = instrument_inline_scripts(SourceId(0), input).unwrap();
        assert_snapshot!(output);
    }

    #[test]
    fn test_instrument_html_inline_script_module_type() {
        let input = indoc! { r#"
        <!DOCTYPE html>
        <html>
        <body>
        <script type="module">
        export function example(a, b, c) {
            return a ? b : c;
        }
        console.log(example(true, 1, 2));
        </script>
        </body>
        </html>
        "# };

        let output = instrument_inline_scripts(SourceId(0), input).unwrap();
        assert_snapshot!(output);
    }

    #[test]
    fn test_instrument_html_template() {
        let input = indoc! { r#"
        <!DOCTYPE html>
        <html>
        <body>
        <template>
        <p>Leave template alone!</p>
        </template>
        </body>
        </html>
        "# };

        let output = instrument_inline_scripts(SourceId(0), input).unwrap();
        assert_snapshot!(output);
    }
}
