use std::path::Path;
use std::{env, fs};

use tdr_tl_gen::{Config, generate};
use tdr_tl_parser::Schema;

fn main() {
    let schema_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).to_path_buf();

    let api_path = schema_dir.join("api.tl");
    let mtproto_path = schema_dir.join("mtproto.tl");
    println!("cargo:rerun-if-changed={}", api_path.display());
    println!("cargo:rerun-if-changed={}", mtproto_path.display());

    let api = Schema::parse(&fs::read_to_string(&api_path).unwrap()).unwrap();
    let layer = api.layer.expect("api.tl has no `// LAYER N` line");
    fs::write(out.join("layer.rs"), layer.to_string()).unwrap();
    fs::write(
        out.join("api.rs"),
        generate(
            &api,
            &Config {
                module_path: "crate",
                runtime_path: "crate",
                string_as_bytes: false,
            },
        ),
    )
    .unwrap();

    let mtproto = Schema::parse(&fs::read_to_string(&mtproto_path).unwrap()).unwrap();
    fs::write(
        out.join("mtproto.rs"),
        generate(
            &mtproto,
            &Config {
                module_path: "crate::mtproto",
                runtime_path: "crate",
                string_as_bytes: true,
            },
        ),
    )
    .unwrap();
}
