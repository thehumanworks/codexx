use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(crate_dir_arg) = std::env::args().nth(1) else {
        eprintln!("Usage: generate-proto <app-server-protocol-crate-dir>");
        std::process::exit(1);
    };

    let crate_dir = PathBuf::from(crate_dir_arg);
    let proto_dir = crate_dir.join("proto");
    let proto_file = proto_dir.join("codex.app_server.v2.proto");
    let out_dir = crate_dir.join("src/proto");

    std::fs::create_dir_all(&out_dir)?;

    tonic_prost_build::configure()
        .build_client(false)
        .build_server(false)
        .out_dir(&out_dir)
        .compile_protos(&[proto_file], &[proto_dir])?;

    normalize_generated_rust(&out_dir.join("codex.app_server.v2.rs"))?;

    Ok(())
}

fn normalize_generated_rust(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let generated = std::fs::read_to_string(path)?;
    let normalized = generated
        .replace("::prost::alloc::string::String", "String")
        .replace("::prost::alloc::vec::Vec", "Vec")
        .replace("::core::option::Option", "Option");

    if normalized != generated {
        std::fs::write(path, normalized)?;
    }

    Ok(())
}
