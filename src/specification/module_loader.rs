use std::{path::PathBuf, rc::Rc};

use crate::specification::result::{Result, SpecificationError};
use boa_engine::{
    module::{MapModuleLoader, ModuleLoader, Referrer, SimpleModuleLoader},
    Context, JsResult, JsString, Module, Source,
};
use include_dir::{include_dir, Dir};

static JS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/target/specification");

pub struct HybridModuleLoader {
    map_loader: Rc<MapModuleLoader>,
    file_loader: Rc<SimpleModuleLoader>,
}

impl HybridModuleLoader {
    pub fn new() -> Result<Self> {
        Ok(HybridModuleLoader {
            map_loader: Rc::new(MapModuleLoader::new()),
            file_loader: Rc::new(SimpleModuleLoader::new(".")?),
        })
    }

    pub fn insert_mapped_module(&self, path: impl AsRef<str>, module: Module) {
        self.map_loader.insert(path, module);
    }

    pub fn insert_file_module(&self, path: PathBuf, module: Module) {
        self.file_loader.insert(path, module);
    }
}

impl ModuleLoader for HybridModuleLoader {
    async fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        specifier: JsString,
        context: &std::cell::RefCell<&mut Context>,
    ) -> JsResult<Module> {
        match self
            .map_loader
            .clone()
            .load_imported_module(referrer.clone(), specifier.clone(), context)
            .await
        {
            Ok(module) => Ok(module),
            Err(_) => {
                self.file_loader
                    .clone()
                    .load_imported_module(referrer, specifier, context)
                    .await
            }
        }
    }
}

pub fn load_bombadil_module(context: &mut Context) -> Result<Module> {
    let index_js = JS_DIR
        .get_file("index.js")
        .expect("index.js not available in build");
    let source = Source::from_bytes(index_js.contents());
    return Module::parse(source, None, context)
        .map_err(SpecificationError::JS);
}
