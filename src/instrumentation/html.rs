use anyhow::{anyhow, Result};
use html5ever::{
    parse_document, serialize, tendril::TendrilSink,
    tree_builder::TreeBuilderOpts, ParseOpts,
};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
use oxc::span::SourceType;
use std::io::{BufReader, BufWriter};

use crate::instrumentation::{js::instrument_source_code, source_id::SourceId};

pub fn instrument_inline_scripts(
    source_id: SourceId,
    input: &str,
) -> Result<String> {
    let opts = ParseOpts {
        tree_builder: TreeBuilderOpts {
            // drop_doctype: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut reader = BufReader::new(input.as_bytes());
    let dom = parse_document(RcDom::default(), opts)
        .from_utf8()
        .read_from(&mut reader)?;

    transform_inline_scripts(source_id, &dom)?;

    let document: SerializableHandle = dom.document.clone().into();

    let mut buffer = Vec::with_capacity(input.len());
    {
        let mut writer = BufWriter::new(&mut buffer);
        serialize(&mut writer, &document, Default::default())?;
    }

    String::from_utf8(buffer).map_err(|err| {
        anyhow!("failed to convert HTML into UTF8 string: {}", err)
    })
}

fn transform_inline_scripts(source_id: SourceId, dom: &RcDom) -> Result<()> {
    let mut scripts_count = 0;
    let mut stack: Vec<Handle> = Vec::new();
    stack.push(dom.document.clone());

    while let Some(node) = stack.pop() {
        if let NodeData::Element { name, attrs, .. } = &node.data
            && name.local.as_ref() == "script" {
                let attrs = attrs.borrow();
                let script_src = attrs
                    .iter()
                    .find(|attr| attr.name.local.as_ref() == "src")
                    .map(|attr| attr.value.to_string());

                let script_type = attrs
                    .iter()
                    .find(|attr| attr.name.local.as_ref() == "type")
                    .map(|attr| attr.value.to_string())
                    .unwrap_or("".to_string());

                let is_inline_javascript = script_src.is_none()
                    && (script_type.is_empty() || script_type == "text/javascript");

                let source_type = if script_type == "module" {
                    SourceType::mjs()
                } else {
                    SourceType::cjs()
                };

                if is_inline_javascript {
                    let text_nodes: Vec<Handle> = node
                        .children
                        .borrow()
                        .iter()
                        .filter(|child| {
                            matches!(child.data, NodeData::Text { .. })
                        })
                        .cloned()
                        .collect();

                    for child in text_nodes {
                        if let NodeData::Text { contents } = &child.data {
                            let original = {
                                let c = contents.borrow();
                                c.to_string()
                            };

                            let transformed = instrument_source_code(
                                // Every inline scripts needs a unique ID.
                                source_id.add(scripts_count),
                                &original,
                                source_type,
                            )?;

                            *contents.borrow_mut() = transformed.into();
                        }
                        scripts_count += 1;
                    }
                }
            }

        for child in node.children.borrow().iter() {
            stack.push(child.clone());
        }
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
}
