use std::{path::PathBuf, rc::Rc};

use boa_engine::{
    builtins::promise::PromiseState, context::ContextBuilder, js_string,
    Context, JsError, JsValue, Module, Source,
};
use result::Result;

use crate::specification::{
    module_loader::{load_bombadil_module, HybridModuleLoader},
    result::SpecificationError,
};

mod module_loader;
mod result;

fn spec_module(context: &mut Context) -> Result<Module> {
    let source = Source::from_bytes(
        r#"
        import { always, condition, eventually, extract, time } from "bombadil";

        // Invariant

        const notification_count = extract(
          (state) => state.document.body.querySelectorAll(".notification").length,
        );

        export const max_notifications_shown = always(
          () => notification_count.current <= 5,
        );
        "#,
    );
    return Module::parse(source, None, context)
        .map_err(SpecificationError::JsError);
}

#[allow(dead_code)]
fn test() -> Result<()> {
    let loader = Rc::new(HybridModuleLoader::new()?);

    // Instantiate the execution context
    let mut context = ContextBuilder::default()
        .module_loader(loader.clone())
        .build()
        .unwrap();

    let bombadil_module = load_bombadil_module(&mut context)?;
    loader.insert_mapped_module("bombadil", bombadil_module.clone());

    let spec_module = spec_module(&mut context)?;
    loader.insert_file_module(PathBuf::from("test.js"), spec_module.clone());

    let promise = spec_module.load_link_evaluate(&mut context);
    context.run_jobs().unwrap();
    let result = promise.state();

    match result {
        PromiseState::Pending => panic!("module didn't execute"),
        PromiseState::Fulfilled(result) => {
            assert_eq!(result, JsValue::undefined());
        }
        PromiseState::Rejected(err) => {
            panic!(
                "failed: {:?}",
                JsError::from_opaque(err).into_erased(&mut context)
            )
        }
    };

    for key in bombadil_module
        .namespace(&mut context)
        .own_property_keys(&mut context)
        .map_err(SpecificationError::JsError)?
    {
        println!("bombadil export: {key:?}");
    }

    let formula_type = bombadil_module
        .namespace(&mut context)
        .get(js_string!("Formula"), &mut context)
        .map_err(SpecificationError::JsError)?;

    let mut properties = vec![];
    for key in spec_module
        .namespace(&mut context)
        .own_property_keys(&mut context)
        .map_err(SpecificationError::JsError)?
    {
        let value = spec_module
            .namespace(&mut context)
            .get(key.clone(), &mut context)
            .map_err(SpecificationError::JsError)?;

        if value.instance_of(&formula_type, &mut context)? {
            properties.push((key, value));
        } else {
            log::debug!("ignoring exported member {key:?}");
        }
    }

    assert_eq!(
        properties
            .iter()
            .map(|(key, _)| key.to_string())
            .collect::<Vec<_>>(),
        vec!["max_notifications_shown"]
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_js() {
        test().unwrap();
    }
}
