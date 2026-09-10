//! Build script compiling protobuf schemas for TrustLedger ingest.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("../../proto/ledger.proto")?;
    Ok(())
}
