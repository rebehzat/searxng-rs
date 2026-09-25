use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let catalog_dir = Path::new(&manifest_dir).join("src/engines/catalog");
    println!("cargo:rerun-if-changed={}", catalog_dir.display());

    let mut modules = fs::read_dir(&catalog_dir)
        .expect("engine catalog directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    modules.sort();

    let generated = modules
        .iter()
        .map(|module| {
            format!(
                "mod {module} {{ include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/engines/catalog/{module}.rs\")); }}\n"
            )
        })
        .chain(std::iter::once(
            "pub fn definitions() -> Vec<CatalogEntry> {\n    vec![\n"
                .to_string(),
        ))
        .chain(modules.iter().map(|module| {
            format!("        {module}::definition(),\n")
        }))
        .chain(std::iter::once("    ]\n}\n".to_string()))
        .collect::<String>();

    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("catalog.rs");
    fs::write(out, generated).expect("write generated engine catalog");
}
