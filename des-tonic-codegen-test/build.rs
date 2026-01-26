fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/foo.proto");
    println!("cargo:rerun-if-changed=proto");

    des_tonic_build::Builder::new().compile(&["proto/foo.proto"], &["proto"])?;
    Ok(())
}
