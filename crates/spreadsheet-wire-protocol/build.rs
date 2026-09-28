use prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let descriptor = "../../proto/wire-descriptor.bin";
    println!("cargo:rerun-if-changed={descriptor}");
    let bytes = std::fs::read(descriptor)?;
    let descriptor = prost_types::FileDescriptorSet::decode(bytes.as_slice())?;
    prost_build::Config::new()
        .boxed(".pxel.wire.v1.CanonicalOperation.kind.set_cell_formula")
        .boxed(".pxel.wire.v1.ClientEnvelope.payload.command")
        .compile_fds(descriptor)?;
    Ok(())
}
