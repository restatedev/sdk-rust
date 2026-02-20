use restate_sdk::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = ingress::Client::new("http://localhost:8080".try_into()?, None)?;
    Ok(())
}
