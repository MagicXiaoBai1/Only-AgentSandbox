use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::var("OUT_DIR")?;
    let out = Path::new(&out_dir);
    // protoc 必须在 PATH（rust-protobuf codegen 调用）。sync 模式（async_all=false）。
    ttrpc_codegen::Codegen::new()
        .out_dir(out)
        .input("proto/shim.proto")
        .include("proto")
        .rust_protobuf()
        .rust_protobuf_customize(ttrpc_codegen::ProtobufCustomize::default().gen_mod_rs(false))
        .customize(ttrpc_codegen::Customize {
            async_all: false,
            ..Default::default()
        })
        .run()?;

    // 生成的 .rs 顶部带 `#![allow(...)]` 内属性，include! 进 `mod {}` 时编译器拒绝
    //（"inner attribute is not permitted in this context"）。剥掉这些行——它们只是 lint
    // 抑制，对本 crate 无影响。
    for f in ["shim.rs", "shim_ttrpc.rs"] {
        let p = out.join(f);
        let s = std::fs::read_to_string(&p)?;
        let stripped: String = s
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("#![") || t.starts_with("//!"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&p, stripped)?;
    }

    println!("cargo::rerun-if-changed=proto/shim.proto");
    Ok(())
}
