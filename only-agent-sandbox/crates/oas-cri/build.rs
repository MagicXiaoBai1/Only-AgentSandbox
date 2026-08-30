fn main() -> Result<(), Box<dyn std::error::Error>> {
    // protoc 必须在 PATH（prost-build 调用）。proto 已剥掉 gogoproto，无外部 import。
    // tonic 0.14 把 proto 编译拆到 tonic-prost-build；默认同时生成 server/client。
    // 注：prost 的 Message/Enumeration 派生宏已自带 Debug 实现，无需额外配置。
    tonic_prost_build::compile_protos("proto/api.proto")?;
    println!("cargo::rerun-if-changed=proto/api.proto");
    Ok(())
}
