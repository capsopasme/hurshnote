//! 嵌入 Windows 资源：图标、manifest、版本信息。

fn main() {
    println!("cargo:rerun-if-changed=res");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("res/hurshnote.rc", embed_resource::NONE);
    }
}
